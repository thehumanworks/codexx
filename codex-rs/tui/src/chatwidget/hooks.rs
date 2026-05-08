use std::path::PathBuf;

use super::ChatWidget;
use crate::app_event::AppEvent;
use crate::bottom_pane::HooksBrowserView;
use codex_app_server_protocol::HookErrorInfo;
use codex_app_server_protocol::HookMetadata;
use codex_app_server_protocol::HooksListResponse;

impl ChatWidget {
    pub(crate) fn add_hooks_output(&mut self) {
        self.app_event_tx.send(AppEvent::FetchHooksList {
            cwd: self.config.cwd.to_path_buf(),
        });
    }

    pub(crate) fn on_hooks_loaded(
        &mut self,
        cwd: PathBuf,
        result: Result<HooksListResponse, String>,
    ) {
        if self.config.cwd.as_path() != cwd.as_path() {
            return;
        }

        match result {
            Ok(response) => {
                let (hooks, warnings, errors) = response
                    .data
                    .into_iter()
                    .find(|entry| entry.cwd.as_path() == cwd.as_path())
                    .map(|entry| (entry.hooks, entry.warnings, entry.errors))
                    .unwrap_or_default();
                self.open_hooks_browser(hooks, warnings, errors);
            }
            Err(err) => self.add_error_message(format!("Failed to load hooks: {err}")),
        }
    }

    pub(crate) fn open_hooks_browser(
        &mut self,
        hooks: Vec<HookMetadata>,
        warnings: Vec<String>,
        errors: Vec<HookErrorInfo>,
    ) {
        self.bottom_pane.show_view(Box::new(HooksBrowserView::new(
            hooks,
            warnings,
            errors,
            self.app_event_tx.clone(),
        )));
        self.request_redraw();
    }
}
