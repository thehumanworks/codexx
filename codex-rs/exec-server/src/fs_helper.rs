use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use codex_app_server_protocol::JSONRPCErrorError;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::io;
use tokio::io::AsyncWriteExt;

use crate::CopyOptions;
use crate::CreateDirectoryOptions;
use crate::ExecutorFileSystem;
use crate::RemoveOptions;
use crate::local_file_system::DirectFileSystem;
use crate::protocol::FS_COPY_METHOD;
use crate::protocol::FS_CREATE_DIRECTORY_METHOD;
use crate::protocol::FS_GET_METADATA_METHOD;
use crate::protocol::FS_READ_DIRECTORY_METHOD;
use crate::protocol::FS_READ_FILE_METHOD;
use crate::protocol::FS_REMOVE_METHOD;
use crate::protocol::FS_WRITE_FILE_METHOD;
use crate::protocol::FsCopyParams;
use crate::protocol::FsCopyResponse;
use crate::protocol::FsCreateDirectoryParams;
use crate::protocol::FsCreateDirectoryResponse;
use crate::protocol::FsGetMetadataParams;
use crate::protocol::FsGetMetadataResponse;
use crate::protocol::FsReadDirectoryEntry;
use crate::protocol::FsReadDirectoryParams;
use crate::protocol::FsReadDirectoryResponse;
use crate::protocol::FsReadFileParams;
use crate::protocol::FsReadFileResponse;
use crate::protocol::FsRemoveParams;
use crate::protocol::FsRemoveResponse;
use crate::protocol::FsWriteFileParams;
use crate::protocol::FsWriteFileResponse;
use crate::rpc::internal_error;
use crate::rpc::invalid_request;
use crate::rpc::not_found;

pub const CODEX_FS_HELPER_ARG1: &str = "--codex-run-as-fs-helper";
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "params")]
pub(crate) enum FsHelperRequest {
    #[serde(rename = "fs/readFile")]
    ReadFile(FsReadFileParams),
    #[serde(rename = "fs/writeFile")]
    WriteFile(FsWriteFileParams),
    #[serde(rename = "fs/createDirectory")]
    CreateDirectory(FsCreateDirectoryParams),
    #[serde(rename = "fs/getMetadata")]
    GetMetadata(FsGetMetadataParams),
    #[serde(rename = "fs/readDirectory")]
    ReadDirectory(FsReadDirectoryParams),
    #[serde(rename = "fs/remove")]
    Remove(FsRemoveParams),
    #[serde(rename = "fs/copy")]
    Copy(FsCopyParams),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "payload", rename_all = "camelCase")]
pub(crate) enum FsHelperResponse {
    Ok(FsHelperPayload),
    Error(JSONRPCErrorError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "response")]
pub(crate) enum FsHelperPayload {
    #[serde(rename = "fs/readFile")]
    ReadFile(FsReadFileResponse),
    #[serde(rename = "fs/writeFile")]
    WriteFile(FsWriteFileResponse),
    #[serde(rename = "fs/createDirectory")]
    CreateDirectory(FsCreateDirectoryResponse),
    #[serde(rename = "fs/getMetadata")]
    GetMetadata(FsGetMetadataResponse),
    #[serde(rename = "fs/readDirectory")]
    ReadDirectory(FsReadDirectoryResponse),
    #[serde(rename = "fs/remove")]
    Remove(FsRemoveResponse),
    #[serde(rename = "fs/copy")]
    Copy(FsCopyResponse),
}

impl FsHelperPayload {
    fn operation(&self) -> &'static str {
        match self {
            Self::ReadFile(_) => FS_READ_FILE_METHOD,
            Self::WriteFile(_) => FS_WRITE_FILE_METHOD,
            Self::CreateDirectory(_) => FS_CREATE_DIRECTORY_METHOD,
            Self::GetMetadata(_) => FS_GET_METADATA_METHOD,
            Self::ReadDirectory(_) => FS_READ_DIRECTORY_METHOD,
            Self::Remove(_) => FS_REMOVE_METHOD,
            Self::Copy(_) => FS_COPY_METHOD,
        }
    }

    pub(crate) fn expect_read_file(self) -> Result<FsReadFileResponse, JSONRPCErrorError> {
        match self {
            Self::ReadFile(response) => Ok(response),
            other => Err(unexpected_response(FS_READ_FILE_METHOD, other.operation())),
        }
    }

    pub(crate) fn expect_write_file(self) -> Result<FsWriteFileResponse, JSONRPCErrorError> {
        match self {
            Self::WriteFile(response) => Ok(response),
            other => Err(unexpected_response(FS_WRITE_FILE_METHOD, other.operation())),
        }
    }

    pub(crate) fn expect_create_directory(
        self,
    ) -> Result<FsCreateDirectoryResponse, JSONRPCErrorError> {
        match self {
            Self::CreateDirectory(response) => Ok(response),
            other => Err(unexpected_response(
                FS_CREATE_DIRECTORY_METHOD,
                other.operation(),
            )),
        }
    }

    pub(crate) fn expect_get_metadata(self) -> Result<FsGetMetadataResponse, JSONRPCErrorError> {
        match self {
            Self::GetMetadata(response) => Ok(response),
            other => Err(unexpected_response(
                FS_GET_METADATA_METHOD,
                other.operation(),
            )),
        }
    }

    pub(crate) fn expect_read_directory(
        self,
    ) -> Result<FsReadDirectoryResponse, JSONRPCErrorError> {
        match self {
            Self::ReadDirectory(response) => Ok(response),
            other => Err(unexpected_response(
                FS_READ_DIRECTORY_METHOD,
                other.operation(),
            )),
        }
    }

    pub(crate) fn expect_remove(self) -> Result<FsRemoveResponse, JSONRPCErrorError> {
        match self {
            Self::Remove(response) => Ok(response),
            other => Err(unexpected_response(FS_REMOVE_METHOD, other.operation())),
        }
    }

    pub(crate) fn expect_copy(self) -> Result<FsCopyResponse, JSONRPCErrorError> {
        match self {
            Self::Copy(response) => Ok(response),
            other => Err(unexpected_response(FS_COPY_METHOD, other.operation())),
        }
    }
}

fn unexpected_response(expected: &str, actual: &str) -> JSONRPCErrorError {
    internal_error(format!(
        "unexpected fs sandbox helper response: expected {expected}, got {actual}"
    ))
}

pub(crate) async fn run_direct_request(
    request: FsHelperRequest,
) -> Result<FsHelperPayload, JSONRPCErrorError> {
    let file_system = DirectFileSystem;
    match request {
        FsHelperRequest::ReadFile(params) => {
            let data = file_system
                .read_file(&params.path, /*sandbox*/ None)
                .await
                .map_err(map_fs_error)?;
            Ok(FsHelperPayload::ReadFile(FsReadFileResponse {
                data_base64: STANDARD.encode(data),
            }))
        }
        FsHelperRequest::WriteFile(params) => {
            let bytes = STANDARD.decode(params.data_base64).map_err(|err| {
                invalid_request(format!(
                    "{FS_WRITE_FILE_METHOD} requires valid base64 dataBase64: {err}"
                ))
            })?;
            write_file_by_replacing_path(&params.path, bytes)
                .await
                .map_err(map_fs_error)?;
            Ok(FsHelperPayload::WriteFile(FsWriteFileResponse {}))
        }
        FsHelperRequest::CreateDirectory(params) => {
            file_system
                .create_directory(
                    &params.path,
                    CreateDirectoryOptions {
                        recursive: params.recursive.unwrap_or(true),
                    },
                    /*sandbox*/ None,
                )
                .await
                .map_err(map_fs_error)?;
            Ok(FsHelperPayload::CreateDirectory(
                FsCreateDirectoryResponse {},
            ))
        }
        FsHelperRequest::GetMetadata(params) => {
            let metadata = file_system
                .get_metadata(&params.path, /*sandbox*/ None)
                .await
                .map_err(map_fs_error)?;
            Ok(FsHelperPayload::GetMetadata(FsGetMetadataResponse {
                is_directory: metadata.is_directory,
                is_file: metadata.is_file,
                is_symlink: metadata.is_symlink,
                created_at_ms: metadata.created_at_ms,
                modified_at_ms: metadata.modified_at_ms,
            }))
        }
        FsHelperRequest::ReadDirectory(params) => {
            let entries = file_system
                .read_directory(&params.path, /*sandbox*/ None)
                .await
                .map_err(map_fs_error)?
                .into_iter()
                .map(|entry| FsReadDirectoryEntry {
                    file_name: entry.file_name,
                    is_directory: entry.is_directory,
                    is_file: entry.is_file,
                })
                .collect();
            Ok(FsHelperPayload::ReadDirectory(FsReadDirectoryResponse {
                entries,
            }))
        }
        FsHelperRequest::Remove(params) => {
            file_system
                .remove(
                    &params.path,
                    RemoveOptions {
                        recursive: params.recursive.unwrap_or(true),
                        force: params.force.unwrap_or(true),
                    },
                    /*sandbox*/ None,
                )
                .await
                .map_err(map_fs_error)?;
            Ok(FsHelperPayload::Remove(FsRemoveResponse {}))
        }
        FsHelperRequest::Copy(params) => {
            file_system
                .copy(
                    &params.source_path,
                    &params.destination_path,
                    CopyOptions {
                        recursive: params.recursive,
                    },
                    /*sandbox*/ None,
                )
                .await
                .map_err(map_fs_error)?;
            Ok(FsHelperPayload::Copy(FsCopyResponse {}))
        }
    }
}

/// The helper is already running inside the platform sandbox. Write through a
/// fresh file and rename it into place so an existing final-path symlink or
/// hard link is replaced rather than used as the write target.
async fn write_file_by_replacing_path(
    path: &codex_utils_absolute_path::AbsolutePathBuf,
    bytes: Vec<u8>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    if path.as_path().file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path has no file name",
        ));
    }

    let (temp_path, mut temp_file) = loop {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_name = format!(".codex-write-{}-{counter}.tmp", std::process::id());
        let temp_path = parent.join(PathBuf::from(temp_name));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp_path.as_path())
            .await
        {
            Ok(file) => break (temp_path, file),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    };

    let write_result = async {
        temp_file.write_all(&bytes).await?;
        temp_file.flush().await
    }
    .await;
    drop(temp_file);
    if let Err(err) = write_result {
        let _ = tokio::fs::remove_file(temp_path.as_path()).await;
        return Err(err);
    }

    if let Ok(metadata) = tokio::fs::metadata(path.as_path()).await {
        let _ = tokio::fs::set_permissions(temp_path.as_path(), metadata.permissions()).await;
    }

    if let Err(err) = replace_with_temp_file(&temp_path, path).await {
        let _ = tokio::fs::remove_file(temp_path.as_path()).await;
        return Err(err);
    }
    Ok(())
}

#[cfg(not(windows))]
async fn replace_with_temp_file(
    temp_path: &codex_utils_absolute_path::AbsolutePathBuf,
    path: &codex_utils_absolute_path::AbsolutePathBuf,
) -> io::Result<()> {
    tokio::fs::rename(temp_path.as_path(), path.as_path()).await
}

#[cfg(windows)]
async fn replace_with_temp_file(
    temp_path: &codex_utils_absolute_path::AbsolutePathBuf,
    path: &codex_utils_absolute_path::AbsolutePathBuf,
) -> io::Result<()> {
    match tokio::fs::rename(temp_path.as_path(), path.as_path()).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            tokio::fs::remove_file(path.as_path()).await?;
            tokio::fs::rename(temp_path.as_path(), path.as_path()).await
        }
        Err(err) => Err(err),
    }
}

fn map_fs_error(err: io::Error) -> JSONRPCErrorError {
    match err.kind() {
        io::ErrorKind::NotFound => not_found(err.to_string()),
        io::ErrorKind::InvalidInput | io::ErrorKind::PermissionDenied => {
            invalid_request(err.to_string())
        }
        _ => internal_error(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_requests_use_fs_method_names() -> serde_json::Result<()> {
        assert_eq!(
            serde_json::to_value(FsHelperRequest::WriteFile(FsWriteFileParams {
                path: std::env::current_dir()
                    .expect("cwd")
                    .join("file")
                    .as_path()
                    .try_into()
                    .expect("absolute path"),
                data_base64: String::new(),
                sandbox: None,
            }))?["operation"],
            FS_WRITE_FILE_METHOD,
        );
        Ok(())
    }
}
