use crate::error_code::internal_error;
use crate::transport::CHANNEL_CAPACITY;
use codex_app_server_protocol::JSONRPCErrorError;
use russh_sftp::protocol::Attrs;
use russh_sftp::protocol::FileAttributes;
use russh_sftp::protocol::Handle;
use russh_sftp::protocol::OpenFlags;
use russh_sftp::protocol::Status;
use russh_sftp::protocol::StatusCode;
use russh_sftp::server::Handler;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::fs::OpenOptions;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::io::SeekFrom;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

const UPLOAD_SFTP_STREAM_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub(crate) struct UploadSftpLane {
    incoming_tx: mpsc::Sender<Vec<u8>>,
}

impl UploadSftpLane {
    pub(crate) async fn start(
        allowed_paths: Arc<Mutex<HashSet<PathBuf>>>,
        binary_writer: mpsc::Sender<Vec<u8>>,
    ) -> Self {
        let (server_stream, bridge_stream) = tokio::io::duplex(UPLOAD_SFTP_STREAM_BUFFER_BYTES);
        russh_sftp::server::run(server_stream, UploadSftpHandler::new(allowed_paths)).await;

        let (mut bridge_reader, mut bridge_writer) = tokio::io::split(bridge_stream);
        let (incoming_tx, mut incoming_rx) = mpsc::channel::<Vec<u8>>(CHANNEL_CAPACITY);

        tokio::spawn(async move {
            while let Some(bytes) = incoming_rx.recv().await {
                if bridge_writer.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            let mut buffer = vec![0; UPLOAD_SFTP_STREAM_BUFFER_BYTES];
            loop {
                match bridge_reader.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(bytes_read) => {
                        if binary_writer
                            .send(buffer[..bytes_read].to_vec())
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });

        Self { incoming_tx }
    }

    pub(crate) async fn send(&self, bytes: Vec<u8>) -> Result<(), JSONRPCErrorError> {
        self.incoming_tx
            .send(bytes)
            .await
            .map_err(|_| internal_error("staged upload SFTP lane is closed".to_string()))
    }
}

struct UploadSftpHandler {
    allowed_paths: Arc<Mutex<HashSet<PathBuf>>>,
    handles: HashMap<String, UploadHandle>,
    next_handle_id: u64,
}

struct UploadHandle {
    file: File,
}

#[derive(Clone, Copy)]
enum UploadSftpError {
    NoSuchFile,
    PermissionDenied,
    Failure,
    Unsupported,
}

impl From<UploadSftpError> for StatusCode {
    fn from(value: UploadSftpError) -> Self {
        match value {
            UploadSftpError::NoSuchFile => Self::NoSuchFile,
            UploadSftpError::PermissionDenied => Self::PermissionDenied,
            UploadSftpError::Failure => Self::Failure,
            UploadSftpError::Unsupported => Self::OpUnsupported,
        }
    }
}

impl From<io::Error> for UploadSftpError {
    fn from(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => Self::NoSuchFile,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            _ => Self::Failure,
        }
    }
}

impl UploadSftpHandler {
    fn new(allowed_paths: Arc<Mutex<HashSet<PathBuf>>>) -> Self {
        Self {
            allowed_paths,
            handles: HashMap::new(),
            next_handle_id: 0,
        }
    }

    async fn is_allowed_path(&self, path: &str) -> bool {
        self.allowed_paths
            .lock()
            .await
            .contains(&PathBuf::from(path))
    }

    fn next_handle(&mut self) -> String {
        let handle = self.next_handle_id.to_string();
        self.next_handle_id = self.next_handle_id.wrapping_add(1);
        handle
    }
}

impl Handler for UploadSftpHandler {
    type Error = UploadSftpError;

    fn unimplemented(&self) -> Self::Error {
        UploadSftpError::Unsupported
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        if has_attributes(&attrs) {
            return Err(UploadSftpError::Unsupported);
        }
        if !self.is_allowed_path(&filename).await {
            return Err(UploadSftpError::PermissionDenied);
        }

        let mut options = OpenOptions::new();
        options.write(pflags.contains(OpenFlags::WRITE));
        options.create(pflags.contains(OpenFlags::CREATE));
        options.truncate(pflags.contains(OpenFlags::TRUNCATE));
        options.append(pflags.contains(OpenFlags::APPEND));
        let file = options.open(&filename).await?;
        let handle = self.next_handle();
        self.handles.insert(handle.clone(), UploadHandle { file });
        Ok(Handle { id, handle })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let upload_handle = self
            .handles
            .get_mut(&handle)
            .ok_or(UploadSftpError::NoSuchFile)?;
        upload_handle.file.seek(SeekFrom::Start(offset)).await?;
        upload_handle.file.write_all(&data).await?;
        Ok(ok_status(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        let mut upload_handle = self
            .handles
            .remove(&handle)
            .ok_or(UploadSftpError::NoSuchFile)?;
        upload_handle.file.flush().await?;
        Ok(ok_status(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        if !self.is_allowed_path(&path).await {
            return Err(UploadSftpError::PermissionDenied);
        }
        let metadata = tokio::fs::metadata(path).await?;
        let mut attrs = FileAttributes::empty();
        attrs.size = Some(metadata.len());
        Ok(Attrs { id, attrs })
    }
}

fn has_attributes(attrs: &FileAttributes) -> bool {
    attrs.size.is_some()
        || attrs.uid.is_some()
        || attrs.user.is_some()
        || attrs.gid.is_some()
        || attrs.group.is_some()
        || attrs.permissions.is_some()
        || attrs.atime.is_some()
        || attrs.mtime.is_some()
}

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_string(),
        language_tag: "en-US".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_sftp::client::SftpSession;
    use tempfile::TempDir;

    #[tokio::test]
    async fn writes_only_paths_allocated_for_uploads() {
        let tempdir = TempDir::new().expect("create temp dir");
        let path = tempdir.path().join("uploads").join("note.txt");
        tokio::fs::create_dir_all(path.parent().expect("path parent"))
            .await
            .expect("create upload parent");
        let allowed_paths = Arc::new(Mutex::new(HashSet::from([path.clone()])));
        let (client_stream, server_stream) = tokio::io::duplex(UPLOAD_SFTP_STREAM_BUFFER_BYTES);
        russh_sftp::server::run(server_stream, UploadSftpHandler::new(allowed_paths)).await;

        let session = SftpSession::new(client_stream)
            .await
            .expect("initialize sftp");
        let mut file = session
            .create(path.to_string_lossy())
            .await
            .expect("open staged upload");
        file.write_all(b"hello").await.expect("write staged upload");
        file.shutdown().await.expect("close staged upload");
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
        let (client_stream, server_stream) = tokio::io::duplex(UPLOAD_SFTP_STREAM_BUFFER_BYTES);
        russh_sftp::server::run(
            server_stream,
            UploadSftpHandler::new(Arc::new(Mutex::new(HashSet::new()))),
        )
        .await;

        let session = SftpSession::new(client_stream)
            .await
            .expect("initialize sftp");
        let err = match session.create(path.to_string_lossy()).await {
            Ok(_) => panic!("path should be rejected"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("Permission denied"));
        assert!(!path.exists());
    }
}
