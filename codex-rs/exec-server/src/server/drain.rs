use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::rpc::invalid_request;
use codex_app_server_protocol::JSONRPCErrorError;

pub(crate) struct DrainState {
    draining: AtomicBool,
    active_rpc_requests: AtomicUsize,
    active_http_requests: AtomicUsize,
}

pub(crate) struct ActiveRpcRequest {
    state: Arc<DrainState>,
}

pub(crate) struct ActiveHttpRequest {
    state: Arc<DrainState>,
}

impl DrainState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            draining: AtomicBool::new(false),
            active_rpc_requests: AtomicUsize::new(0),
            active_http_requests: AtomicUsize::new(0),
        })
    }

    pub(crate) fn begin(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    pub(crate) fn try_start_process(&self) -> Result<(), JSONRPCErrorError> {
        if self.is_draining() {
            return Err(invalid_request(
                "exec-server is draining; new processes are not accepted".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn start_rpc_request(self: &Arc<Self>) -> ActiveRpcRequest {
        self.active_rpc_requests.fetch_add(1, Ordering::SeqCst);
        ActiveRpcRequest {
            state: Arc::clone(self),
        }
    }

    pub(crate) fn try_start_http_request(
        self: &Arc<Self>,
    ) -> Result<ActiveHttpRequest, JSONRPCErrorError> {
        if self.is_draining() {
            return Err(invalid_request(
                "exec-server is draining; new HTTP requests are not accepted".to_string(),
            ));
        }
        self.active_http_requests.fetch_add(1, Ordering::SeqCst);
        if self.is_draining() {
            self.active_http_requests.fetch_sub(1, Ordering::SeqCst);
            return Err(invalid_request(
                "exec-server is draining; new HTTP requests are not accepted".to_string(),
            ));
        }
        Ok(ActiveHttpRequest {
            state: Arc::clone(self),
        })
    }

    pub(crate) fn active_http_request_count(&self) -> usize {
        self.active_http_requests.load(Ordering::SeqCst)
    }

    pub(crate) fn active_rpc_request_count(&self) -> usize {
        self.active_rpc_requests.load(Ordering::SeqCst)
    }
}

impl Drop for ActiveRpcRequest {
    fn drop(&mut self) {
        self.state
            .active_rpc_requests
            .fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for ActiveHttpRequest {
    fn drop(&mut self) {
        self.state
            .active_http_requests
            .fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::DrainState;

    #[test]
    fn rpc_request_guard_tracks_active_count_until_drop() {
        let state = DrainState::new();
        assert_eq!(state.active_rpc_request_count(), 0);

        let guard = state.start_rpc_request();
        assert_eq!(state.active_rpc_request_count(), 1);

        drop(guard);
        assert_eq!(state.active_rpc_request_count(), 0);
    }
}
