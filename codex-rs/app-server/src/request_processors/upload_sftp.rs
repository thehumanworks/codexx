use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::JSONRPCErrorError;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::fs::OpenOptions;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::io::SeekFrom;
use tokio::sync::Mutex;

const SSH_FXP_INIT: u8 = 1;
const SSH_FXP_VERSION: u8 = 2;
const SSH_FXP_OPEN: u8 = 3;
const SSH_FXP_CLOSE: u8 = 4;
const SSH_FXP_WRITE: u8 = 6;
const SSH_FXP_STAT: u8 = 17;
const SSH_FXP_STATUS: u8 = 101;
const SSH_FXP_HANDLE: u8 = 102;
const SSH_FXP_ATTRS: u8 = 105;

const SSH_FX_OK: u32 = 0;
const SSH_FX_NO_SUCH_FILE: u32 = 2;
const SSH_FX_PERMISSION_DENIED: u32 = 3;
const SSH_FX_OP_UNSUPPORTED: u32 = 8;

const SSH_FXF_WRITE: u32 = 0x0000_0002;
const SSH_FXF_APPEND: u32 = 0x0000_0004;
const SSH_FXF_CREAT: u32 = 0x0000_0008;
const SSH_FXF_TRUNC: u32 = 0x0000_0010;

const SSH_FILEXFER_ATTR_SIZE: u32 = 0x0000_0001;
const MAX_SFTP_PACKET_BYTES: usize = 1024 * 1024;

pub(crate) struct UploadSftpSession {
    allowed_paths: Arc<Mutex<HashSet<PathBuf>>>,
    pending_bytes: Vec<u8>,
    handles: HashMap<Vec<u8>, UploadHandle>,
    next_handle_id: u64,
}

struct UploadHandle {
    file: File,
}

impl UploadSftpSession {
    pub(crate) fn new(allowed_paths: Arc<Mutex<HashSet<PathBuf>>>) -> Self {
        Self {
            allowed_paths,
            pending_bytes: Vec::new(),
            handles: HashMap::new(),
            next_handle_id: 0,
        }
    }

    pub(crate) async fn process_bytes(
        &mut self,
        bytes: Vec<u8>,
    ) -> Result<Vec<Vec<u8>>, JSONRPCErrorError> {
        self.pending_bytes.extend(bytes);
        let mut responses = Vec::new();
        while let Some(packet) = take_packet(&mut self.pending_bytes)? {
            if let Some(response) = self.process_packet(packet).await? {
                responses.push(response);
            }
        }
        Ok(responses)
    }

    async fn process_packet(
        &mut self,
        packet: Vec<u8>,
    ) -> Result<Option<Vec<u8>>, JSONRPCErrorError> {
        let mut cursor = PacketCursor::new(&packet);
        let packet_type = cursor.read_u8()?;
        match packet_type {
            SSH_FXP_INIT => {
                let _client_version = cursor.read_u32()?;
                cursor.finish()?;
                Ok(Some(version_packet()))
            }
            SSH_FXP_OPEN => {
                let request_id = cursor.read_u32()?;
                let path = cursor.read_string_utf8()?;
                let pflags = cursor.read_u32()?;
                let attrs = cursor.read_u32()?;
                cursor.finish()?;
                if attrs != 0 {
                    return Ok(Some(status_packet(
                        request_id,
                        SSH_FX_OP_UNSUPPORTED,
                        "open attributes are not supported",
                    )));
                }
                if !self.is_allowed_path(&path).await {
                    return Ok(Some(status_packet(
                        request_id,
                        SSH_FX_PERMISSION_DENIED,
                        "path is not a staged upload",
                    )));
                }
                let mut options = OpenOptions::new();
                options.write(pflags & SSH_FXF_WRITE != 0);
                options.create(pflags & SSH_FXF_CREAT != 0);
                options.truncate(pflags & SSH_FXF_TRUNC != 0);
                options.append(pflags & SSH_FXF_APPEND != 0);
                match options.open(&path).await {
                    Ok(file) => {
                        let handle = self.next_handle();
                        self.handles.insert(handle.clone(), UploadHandle { file });
                        Ok(Some(handle_packet(request_id, &handle)))
                    }
                    Err(err) => Ok(Some(status_packet(
                        request_id,
                        map_status_code(&err),
                        &err.to_string(),
                    ))),
                }
            }
            SSH_FXP_WRITE => {
                let request_id = cursor.read_u32()?;
                let handle = cursor.read_string_bytes()?;
                let offset = cursor.read_u64()?;
                let data = cursor.read_string_bytes()?;
                cursor.finish()?;
                let Some(upload_handle) = self.handles.get_mut(&handle) else {
                    return Ok(Some(status_packet(
                        request_id,
                        SSH_FX_NO_SUCH_FILE,
                        "unknown upload handle",
                    )));
                };
                if let Err(err) = upload_handle.file.seek(SeekFrom::Start(offset)).await {
                    return Ok(Some(status_packet(
                        request_id,
                        map_status_code(&err),
                        &err.to_string(),
                    )));
                }
                match upload_handle.file.write_all(&data).await {
                    Ok(()) => Ok(Some(status_packet(request_id, SSH_FX_OK, "Ok"))),
                    Err(err) => Ok(Some(status_packet(
                        request_id,
                        map_status_code(&err),
                        &err.to_string(),
                    ))),
                }
            }
            SSH_FXP_CLOSE => {
                let request_id = cursor.read_u32()?;
                let handle = cursor.read_string_bytes()?;
                cursor.finish()?;
                let Some(mut upload_handle) = self.handles.remove(&handle) else {
                    return Ok(Some(status_packet(
                        request_id,
                        SSH_FX_NO_SUCH_FILE,
                        "unknown upload handle",
                    )));
                };
                match upload_handle.file.flush().await {
                    Ok(()) => Ok(Some(status_packet(request_id, SSH_FX_OK, "Ok"))),
                    Err(err) => Ok(Some(status_packet(
                        request_id,
                        map_status_code(&err),
                        &err.to_string(),
                    ))),
                }
            }
            SSH_FXP_STAT => {
                let request_id = cursor.read_u32()?;
                let path = cursor.read_string_utf8()?;
                cursor.finish()?;
                if !self.is_allowed_path(&path).await {
                    return Ok(Some(status_packet(
                        request_id,
                        SSH_FX_PERMISSION_DENIED,
                        "path is not a staged upload",
                    )));
                }
                match tokio::fs::metadata(path).await {
                    Ok(metadata) => Ok(Some(attrs_packet(request_id, metadata.len()))),
                    Err(err) => Ok(Some(status_packet(
                        request_id,
                        map_status_code(&err),
                        &err.to_string(),
                    ))),
                }
            }
            _ => {
                let request_id = cursor.read_u32().unwrap_or_default();
                Ok(Some(status_packet(
                    request_id,
                    SSH_FX_OP_UNSUPPORTED,
                    "operation is not supported",
                )))
            }
        }
    }

    async fn is_allowed_path(&self, path: &str) -> bool {
        self.allowed_paths
            .lock()
            .await
            .contains(&PathBuf::from(path))
    }

    fn next_handle(&mut self) -> Vec<u8> {
        let handle = self.next_handle_id.to_be_bytes().to_vec();
        self.next_handle_id = self.next_handle_id.wrapping_add(1);
        handle
    }
}

fn take_packet(buffer: &mut Vec<u8>) -> Result<Option<Vec<u8>>, JSONRPCErrorError> {
    if buffer.len() < 4 {
        return Ok(None);
    }
    let packet_len = u32::from_be_bytes(
        buffer[..4]
            .try_into()
            .map_err(|_| invalid_request("invalid SFTP packet length prefix".to_string()))?,
    ) as usize;
    if packet_len > MAX_SFTP_PACKET_BYTES {
        return Err(invalid_sftp_packet("packet exceeds 1 MiB"));
    }
    if buffer.len() < packet_len + 4 {
        return Ok(None);
    }
    let packet = buffer[4..packet_len + 4].to_vec();
    buffer.drain(..packet_len + 4);
    Ok(Some(packet))
}

fn version_packet() -> Vec<u8> {
    packet(|payload| {
        payload.push(SSH_FXP_VERSION);
        payload.extend_from_slice(&3_u32.to_be_bytes());
    })
}

fn status_packet(request_id: u32, status_code: u32, message: &str) -> Vec<u8> {
    packet(|payload| {
        payload.push(SSH_FXP_STATUS);
        payload.extend_from_slice(&request_id.to_be_bytes());
        payload.extend_from_slice(&status_code.to_be_bytes());
        push_string(payload, message.as_bytes());
        push_string(payload, b"en-US");
    })
}

fn handle_packet(request_id: u32, handle: &[u8]) -> Vec<u8> {
    packet(|payload| {
        payload.push(SSH_FXP_HANDLE);
        payload.extend_from_slice(&request_id.to_be_bytes());
        push_string(payload, handle);
    })
}

fn attrs_packet(request_id: u32, size: u64) -> Vec<u8> {
    packet(|payload| {
        payload.push(SSH_FXP_ATTRS);
        payload.extend_from_slice(&request_id.to_be_bytes());
        payload.extend_from_slice(&SSH_FILEXFER_ATTR_SIZE.to_be_bytes());
        payload.extend_from_slice(&size.to_be_bytes());
    })
}

fn packet(build_payload: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut payload = Vec::new();
    build_payload(&mut payload);
    let mut packet = Vec::with_capacity(payload.len() + 4);
    packet.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    packet.extend_from_slice(&payload);
    packet
}

fn push_string(payload: &mut Vec<u8>, value: &[u8]) {
    payload.extend_from_slice(&(value.len() as u32).to_be_bytes());
    payload.extend_from_slice(value);
}

fn map_status_code(err: &io::Error) -> u32 {
    match err.kind() {
        io::ErrorKind::NotFound => SSH_FX_NO_SUCH_FILE,
        io::ErrorKind::PermissionDenied => SSH_FX_PERMISSION_DENIED,
        _ => SSH_FX_OP_UNSUPPORTED,
    }
}

struct PacketCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> PacketCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, JSONRPCErrorError> {
        let value = *self
            .bytes
            .get(self.offset)
            .ok_or_else(|| invalid_sftp_packet("missing u8"))?;
        self.offset += 1;
        Ok(value)
    }

    fn read_u32(&mut self) -> Result<u32, JSONRPCErrorError> {
        let bytes = self
            .bytes
            .get(self.offset..self.offset + 4)
            .ok_or_else(|| invalid_sftp_packet("missing u32"))?;
        self.offset += 4;
        Ok(u32::from_be_bytes(bytes.try_into().map_err(|_| {
            internal_error("failed to decode SFTP u32".to_string())
        })?))
    }

    fn read_u64(&mut self) -> Result<u64, JSONRPCErrorError> {
        let bytes = self
            .bytes
            .get(self.offset..self.offset + 8)
            .ok_or_else(|| invalid_sftp_packet("missing u64"))?;
        self.offset += 8;
        Ok(u64::from_be_bytes(bytes.try_into().map_err(|_| {
            internal_error("failed to decode SFTP u64".to_string())
        })?))
    }

    fn read_string_bytes(&mut self) -> Result<Vec<u8>, JSONRPCErrorError> {
        let len = self.read_u32()? as usize;
        let bytes = self
            .bytes
            .get(self.offset..self.offset + len)
            .ok_or_else(|| invalid_sftp_packet("truncated string"))?;
        self.offset += len;
        Ok(bytes.to_vec())
    }

    fn read_string_utf8(&mut self) -> Result<String, JSONRPCErrorError> {
        String::from_utf8(self.read_string_bytes()?)
            .map_err(|_| invalid_sftp_packet("path must be UTF-8"))
    }

    fn finish(&self) -> Result<(), JSONRPCErrorError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid_sftp_packet("unexpected trailing bytes"))
        }
    }
}

fn invalid_sftp_packet(message: &str) -> JSONRPCErrorError {
    invalid_request(format!("invalid staged upload SFTP packet: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn writes_only_paths_allocated_for_uploads() {
        let tempdir = TempDir::new().expect("create temp dir");
        let path = tempdir.path().join("uploads").join("note.txt");
        tokio::fs::create_dir_all(path.parent().expect("path parent"))
            .await
            .expect("create upload parent");
        let allowed_paths = Arc::new(Mutex::new(HashSet::from([path.clone()])));
        let mut session = UploadSftpSession::new(allowed_paths);

        let responses = session
            .process_bytes(init_packet())
            .await
            .expect("init packet should succeed");
        assert_eq!(packet_type(&responses[0]), SSH_FXP_VERSION);

        let responses = session
            .process_bytes(open_packet(
                /*request_id*/ 1,
                &path,
                SSH_FXF_WRITE | SSH_FXF_CREAT | SSH_FXF_TRUNC,
            ))
            .await
            .expect("open packet should succeed");
        assert_eq!(packet_type(&responses[0]), SSH_FXP_HANDLE);
        let handle = response_handle(&responses[0]);

        let responses = session
            .process_bytes(write_packet(
                /*request_id*/ 2, &handle, /*offset*/ 0, b"hello",
            ))
            .await
            .expect("write packet should succeed");
        assert_eq!(response_status(&responses[0]), SSH_FX_OK);

        let responses = session
            .process_bytes(close_packet(/*request_id*/ 3, &handle))
            .await
            .expect("close packet should succeed");
        assert_eq!(response_status(&responses[0]), SSH_FX_OK);
        assert_eq!(
            tokio::fs::read_to_string(path).await.expect("read upload"),
            "hello"
        );
    }

    #[tokio::test]
    async fn rejects_paths_that_were_not_allocated() {
        let tempdir = TempDir::new().expect("create temp dir");
        let path = tempdir.path().join("uploads").join("note.txt");
        tokio::fs::create_dir_all(path.parent().expect("path parent"))
            .await
            .expect("create upload parent");
        let mut session = UploadSftpSession::new(Arc::new(Mutex::new(HashSet::new())));

        let responses = session
            .process_bytes(open_packet(
                /*request_id*/ 1,
                &path,
                SSH_FXF_WRITE | SSH_FXF_CREAT | SSH_FXF_TRUNC,
            ))
            .await
            .expect("open packet should be handled");
        assert_eq!(response_status(&responses[0]), SSH_FX_PERMISSION_DENIED);
        assert!(!path.exists());
    }

    fn init_packet() -> Vec<u8> {
        packet(|payload| {
            payload.push(SSH_FXP_INIT);
            payload.extend_from_slice(&3_u32.to_be_bytes());
        })
    }

    fn open_packet(request_id: u32, path: &std::path::Path, flags: u32) -> Vec<u8> {
        packet(|payload| {
            payload.push(SSH_FXP_OPEN);
            payload.extend_from_slice(&request_id.to_be_bytes());
            push_string(payload, path.to_string_lossy().as_bytes());
            payload.extend_from_slice(&flags.to_be_bytes());
            payload.extend_from_slice(&0_u32.to_be_bytes());
        })
    }

    fn write_packet(request_id: u32, handle: &[u8], offset: u64, data: &[u8]) -> Vec<u8> {
        packet(|payload| {
            payload.push(SSH_FXP_WRITE);
            payload.extend_from_slice(&request_id.to_be_bytes());
            push_string(payload, handle);
            payload.extend_from_slice(&offset.to_be_bytes());
            push_string(payload, data);
        })
    }

    fn close_packet(request_id: u32, handle: &[u8]) -> Vec<u8> {
        packet(|payload| {
            payload.push(SSH_FXP_CLOSE);
            payload.extend_from_slice(&request_id.to_be_bytes());
            push_string(payload, handle);
        })
    }

    fn packet_type(packet: &[u8]) -> u8 {
        packet[4]
    }

    fn response_status(packet: &[u8]) -> u32 {
        u32::from_be_bytes(packet[9..13].try_into().expect("status code bytes"))
    }

    fn response_handle(packet: &[u8]) -> Vec<u8> {
        let len = u32::from_be_bytes(packet[9..13].try_into().expect("handle len")) as usize;
        packet[13..13 + len].to_vec()
    }
}
