mod helpers;
mod list_threads;

use async_trait::async_trait;
use codex_protocol::ThreadId;

use crate::AppendThreadItemsParams;
use crate::ArchiveThreadParams;
use crate::CreateThreadParams;
use crate::ListThreadsParams;
use crate::LoadThreadHistoryParams;
use crate::ReadThreadByRolloutPathParams;
use crate::ReadThreadParams;
use crate::ResumeThreadParams;
use crate::StoredThread;
use crate::StoredThreadHistory;
use crate::ThreadPage;
use crate::ThreadStore;
use crate::ThreadStoreError;
use crate::ThreadStoreResult;
use crate::UpdateThreadMetadataParams;
use proto::thread_store_client::ThreadStoreClient;
use tonic::codegen::InterceptedService;
use tonic::metadata::BinaryMetadataValue;
use tonic::service::Interceptor;
use tonic::transport::Channel;
use tonic::transport::Endpoint;

#[path = "proto/codex.thread_store.v1.rs"]
mod proto;

/// Metadata key used to forward the app-server's opaque identity key to remote contracts.
pub const IDENTITY_KEY_HEADER: &str = "x-codex-app-server-identity-key-bin";

#[derive(Clone, Debug)]
struct IdentityKeyInterceptor {
    identity_key: Option<Vec<u8>>,
}

impl Interceptor for IdentityKeyInterceptor {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(identity_key) = &self.identity_key {
            request.metadata_mut().insert_bin(
                IDENTITY_KEY_HEADER,
                BinaryMetadataValue::from_bytes(identity_key),
            );
        }
        Ok(request)
    }
}

type RemoteThreadStoreClient =
    ThreadStoreClient<InterceptedService<Channel, IdentityKeyInterceptor>>;

/// gRPC-backed [`ThreadStore`] implementation for deployments whose durable thread data lives
/// outside the app-server process.
///
/// This store is still a work in progress: app-server code should call the generic
/// [`ThreadStore`] methods, and unsupported remote operations will return explicit
/// `not_implemented` errors until the remote API catches up.
#[derive(Clone, Debug)]
pub struct RemoteThreadStore {
    endpoint: String,
    identity_key: Option<Vec<u8>>,
}

impl RemoteThreadStore {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            identity_key: None,
        }
    }

    pub fn new_with_identity_key(
        endpoint: impl Into<String>,
        identity_key: Option<Vec<u8>>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            identity_key,
        }
    }

    async fn client(&self) -> ThreadStoreResult<RemoteThreadStoreClient> {
        let channel = Endpoint::new(self.endpoint.clone())
            .map_err(|err| ThreadStoreError::InvalidRequest {
                message: format!("invalid remote thread store endpoint: {err}"),
            })?
            .connect()
            .await
            .map_err(|err| ThreadStoreError::Internal {
                message: format!("failed to connect to remote thread store: {err}"),
            })?;
        Ok(ThreadStoreClient::with_interceptor(
            channel,
            IdentityKeyInterceptor {
                identity_key: self.identity_key.clone(),
            },
        ))
    }
}

#[async_trait]
impl ThreadStore for RemoteThreadStore {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn create_thread(&self, params: CreateThreadParams) -> ThreadStoreResult<()> {
        let thread_id = params.thread_id;
        let request = proto::CreateThreadRequest {
            thread_id: thread_id.to_string(),
            forked_from_id: params.forked_from_id.map(|thread_id| thread_id.to_string()),
            source: Some(helpers::proto_session_source(&params.source)),
            base_instructions_json: helpers::base_instructions_json(&params.base_instructions)?,
            dynamic_tools_json: helpers::dynamic_tools_json(&params.dynamic_tools)?,
            event_persistence_mode: helpers::proto_event_persistence_mode(
                params.event_persistence_mode,
            )
            .into(),
        };
        self.client()
            .await?
            .create_thread(request)
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?;
        Ok(())
    }

    async fn resume_thread(&self, params: ResumeThreadParams) -> ThreadStoreResult<()> {
        let thread_id = params.thread_id;
        let (has_history, history_json) = match params.history {
            Some(history) => (true, helpers::rollout_items_json(&history)?),
            None => (false, Vec::new()),
        };
        let request = proto::ResumeThreadRequest {
            thread_id: thread_id.to_string(),
            rollout_path: params
                .rollout_path
                .map(|path| path.to_string_lossy().into_owned()),
            history_json,
            has_history,
            include_archived: params.include_archived,
            event_persistence_mode: helpers::proto_event_persistence_mode(
                params.event_persistence_mode,
            )
            .into(),
        };
        self.client()
            .await?
            .resume_thread(request)
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?;
        Ok(())
    }

    async fn append_items(&self, params: AppendThreadItemsParams) -> ThreadStoreResult<()> {
        let thread_id = params.thread_id;
        let request = proto::AppendThreadItemsRequest {
            thread_id: thread_id.to_string(),
            items_json: helpers::rollout_items_json(&params.items)?,
        };
        self.client()
            .await?
            .append_items(request)
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?;
        Ok(())
    }

    async fn persist_thread(&self, thread_id: ThreadId) -> ThreadStoreResult<()> {
        self.client()
            .await?
            .persist_thread(helpers::proto_thread_id_request(thread_id))
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?;
        Ok(())
    }

    async fn flush_thread(&self, thread_id: ThreadId) -> ThreadStoreResult<()> {
        self.client()
            .await?
            .flush_thread(helpers::proto_thread_id_request(thread_id))
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?;
        Ok(())
    }

    async fn shutdown_thread(&self, thread_id: ThreadId) -> ThreadStoreResult<()> {
        self.client()
            .await?
            .shutdown_thread(helpers::proto_thread_id_request(thread_id))
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?;
        Ok(())
    }

    async fn discard_thread(&self, thread_id: ThreadId) -> ThreadStoreResult<()> {
        self.client()
            .await?
            .discard_thread(helpers::proto_thread_id_request(thread_id))
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?;
        Ok(())
    }

    async fn load_history(
        &self,
        params: LoadThreadHistoryParams,
    ) -> ThreadStoreResult<StoredThreadHistory> {
        let thread_id = params.thread_id;
        let response = self
            .client()
            .await?
            .load_history(proto::LoadThreadHistoryRequest {
                thread_id: thread_id.to_string(),
                include_archived: params.include_archived,
            })
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?
            .into_inner();
        helpers::stored_thread_history_from_proto(response)
    }

    async fn read_thread(&self, params: ReadThreadParams) -> ThreadStoreResult<StoredThread> {
        let thread_id = params.thread_id;
        let response = self
            .client()
            .await?
            .read_thread(proto::ReadThreadRequest {
                thread_id: thread_id.to_string(),
                include_archived: params.include_archived,
                include_history: params.include_history,
            })
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?
            .into_inner();
        let thread = response.thread.ok_or_else(|| ThreadStoreError::Internal {
            message: "remote thread store omitted read_thread response thread".to_string(),
        })?;
        helpers::stored_thread_from_proto(thread)
    }

    async fn read_thread_by_rollout_path(
        &self,
        _params: ReadThreadByRolloutPathParams,
    ) -> ThreadStoreResult<StoredThread> {
        Err(ThreadStoreError::Internal {
            message: "remote thread store does not support read_thread_by_rollout_path".to_string(),
        })
    }

    async fn list_threads(&self, params: ListThreadsParams) -> ThreadStoreResult<ThreadPage> {
        list_threads::list_threads(self, params).await
    }

    async fn update_thread_metadata(
        &self,
        params: UpdateThreadMetadataParams,
    ) -> ThreadStoreResult<StoredThread> {
        let thread_id = params.thread_id;
        let response = self
            .client()
            .await?
            .update_thread_metadata(proto::UpdateThreadMetadataRequest {
                thread_id: thread_id.to_string(),
                patch: Some(helpers::proto_metadata_patch(params.patch)),
                include_archived: params.include_archived,
            })
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?
            .into_inner();
        let thread = response.thread.ok_or_else(|| ThreadStoreError::Internal {
            message: "remote thread store omitted update_thread_metadata response thread"
                .to_string(),
        })?;
        helpers::stored_thread_from_proto(thread)
    }

    async fn archive_thread(&self, params: ArchiveThreadParams) -> ThreadStoreResult<()> {
        let thread_id = params.thread_id;
        self.client()
            .await?
            .archive_thread(proto::ArchiveThreadRequest {
                thread_id: thread_id.to_string(),
            })
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?;
        Ok(())
    }

    async fn unarchive_thread(
        &self,
        params: ArchiveThreadParams,
    ) -> ThreadStoreResult<StoredThread> {
        let thread_id = params.thread_id;
        let response = self
            .client()
            .await?
            .unarchive_thread(proto::ArchiveThreadRequest {
                thread_id: thread_id.to_string(),
            })
            .await
            .map_err(|status| helpers::remote_status_to_thread_error(status, thread_id))?
            .into_inner();
        let thread = response.thread.ok_or_else(|| ThreadStoreError::Internal {
            message: "remote thread store omitted unarchive_thread response thread".to_string(),
        })?;
        helpers::stored_thread_from_proto(thread)
    }
}
