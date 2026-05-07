use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use codex_code_mode::CodeModeTurnHost;
use codex_code_mode::ExecuteRequest;
use codex_code_mode::RuntimeResponse;
use codex_code_mode::WaitOutcome;
use codex_code_mode::WaitRequest;
use serde_json::Value as JsonValue;

/// Host-facing execution boundary for code-mode runtimes.
///
/// Implementations own how JavaScript execution is reached, while callers keep
/// the model-facing host behavior in `codex-core`. The initial implementation is
/// in-process; later implementations can forward the same requests to a child
/// process without changing the call sites that manage turns.
pub(super) trait CodeModeBackend: Send + Sync {
    fn allocate_cell_id(&self) -> String;

    fn execute(
        &self,
        request: ExecuteRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RuntimeResponse, String>> + Send + '_>>;

    fn wait(
        &self,
        request: WaitRequest,
    ) -> Pin<Box<dyn Future<Output = Result<WaitOutcome, String>> + Send + '_>>;

    fn start_turn_worker(&self, host: Arc<dyn CodeModeTurnHost>) -> CodeModeTurnWorker;
}

/// Opaque turn-scoped worker guard returned by code-mode backends.
///
/// The host only needs to keep this guard alive until the turn ends. Hiding the
/// concrete type keeps the public host seam independent of whether the backend
/// is local or process-backed.
pub(crate) struct CodeModeTurnWorker {
    _inner: Box<dyn TurnWorkerHandle>,
}

impl CodeModeTurnWorker {
    fn new(inner: impl TurnWorkerHandle + 'static) -> Self {
        Self {
            _inner: Box::new(inner),
        }
    }
}

trait TurnWorkerHandle: Send {}

impl<T> TurnWorkerHandle for T where T: Send {}

pub(super) struct InProcessCodeModeBackend {
    inner: codex_code_mode::CodeModeService,
}

impl InProcessCodeModeBackend {
    pub(super) fn new() -> Self {
        Self {
            inner: codex_code_mode::CodeModeService::new(),
        }
    }
}

impl Default for InProcessCodeModeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeModeBackend for InProcessCodeModeBackend {
    fn allocate_cell_id(&self) -> String {
        self.inner.allocate_cell_id()
    }

    fn execute(
        &self,
        request: ExecuteRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RuntimeResponse, String>> + Send + '_>> {
        Box::pin(self.inner.execute(request))
    }

    fn wait(
        &self,
        request: WaitRequest,
    ) -> Pin<Box<dyn Future<Output = Result<WaitOutcome, String>> + Send + '_>> {
        Box::pin(self.inner.wait(request))
    }

    fn start_turn_worker(&self, host: Arc<dyn CodeModeTurnHost>) -> CodeModeTurnWorker {
        CodeModeTurnWorker::new(self.inner.start_turn_worker(host))
    }
}

pub(super) type StoredValues = HashMap<String, JsonValue>;
