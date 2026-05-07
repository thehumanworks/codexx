//! App-layer handlers for the worktree TUI flow.

use codex_worktree::DirtyPolicy;
use codex_worktree::WorktreeInfo;
use codex_worktree::WorktreeListQuery;
use codex_worktree::WorktreeRemoveRequest;
use codex_worktree::WorktreeRequest;
use codex_worktree::WorktreeSource;
use std::path::PathBuf;

use super::*;

impl App {
    pub(super) fn open_worktree_picker(&mut self, tui: &mut tui::Tui) {
        if self.remote_app_server_url.is_some() {
            self.chat_widget.add_error_message(
                "/worktree is not supported for remote sessions yet.".to_string(),
            );
            return;
        }
        self.chat_widget
            .show_selection_view(crate::worktree::loading_params(
                tui.frame_requester(),
                self.config.animations,
            ));
        self.fetch_worktrees_for_picker();
    }

    pub(super) fn on_worktrees_loaded(
        &mut self,
        cwd: PathBuf,
        result: Result<Vec<WorktreeInfo>, String>,
    ) {
        if cwd.as_path() != self.config.cwd.as_path() {
            return;
        }
        let params = match result {
            Ok(entries) if entries.is_empty() => crate::worktree::empty_params(),
            Ok(entries) => crate::worktree::picker_params(entries, self.config.cwd.as_path()),
            Err(err) => crate::worktree::error_params(err),
        };
        self.replace_worktree_view(params);
    }

    pub(super) async fn create_worktree_and_switch(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        branch: String,
        base_ref: Option<String>,
        dirty_policy: Option<DirtyPolicy>,
    ) {
        if self.remote_app_server_url.is_some() {
            self.chat_widget.add_error_message(
                "/worktree is not supported for remote sessions yet.".to_string(),
            );
            return;
        }
        let dirty_policy = match dirty_policy {
            Some(policy) => policy,
            None => match codex_worktree::dirty_state(self.config.cwd.as_path()) {
                Ok(state) if state.is_dirty() => {
                    let params = crate::worktree::dirty_policy_prompt_params(branch, base_ref);
                    self.chat_widget.show_selection_view(params);
                    return;
                }
                Ok(_) => DirtyPolicy::Fail,
                Err(err) => {
                    self.chat_widget
                        .add_error_message(format!("Failed to inspect source checkout: {err}"));
                    return;
                }
            },
        };

        let resolution = match codex_worktree::ensure_worktree(WorktreeRequest {
            codex_home: self.config.codex_home.to_path_buf(),
            source_cwd: self.config.cwd.to_path_buf(),
            branch,
            base_ref,
            dirty_policy,
        }) {
            Ok(resolution) => resolution,
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to create worktree: {err}"));
                return;
            }
        };
        let warnings = resolution
            .warnings
            .iter()
            .map(|warning| warning.message.clone())
            .collect::<Vec<_>>();
        let target = resolution
            .info
            .branch
            .clone()
            .unwrap_or_else(|| resolution.info.name.clone());
        self.show_worktree_switching_view(tui, target).await;
        self.switch_to_worktree_info(tui, app_server, resolution.info)
            .await;
        for warning in warnings {
            self.chat_widget.add_info_message(warning, /*hint*/ None);
        }
    }

    pub(super) async fn switch_to_worktree_target(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        target: String,
    ) {
        if self.remote_app_server_url.is_some() {
            self.chat_widget.add_error_message(
                "/worktree is not supported for remote sessions yet.".to_string(),
            );
            return;
        }
        self.show_worktree_switching_view(tui, target.clone()).await;
        let entries = match self.list_current_repo_worktrees() {
            Ok(entries) => entries,
            Err(err) => {
                self.show_worktree_error("Failed to list worktrees.".to_string(), err.to_string());
                return;
            }
        };
        let info = match crate::worktree::find_worktree(&entries, &target) {
            Ok(info) => info.clone(),
            Err(err) => {
                self.show_worktree_error("Failed to switch worktree.".to_string(), err);
                return;
            }
        };
        self.switch_to_worktree_info(tui, app_server, info).await;
    }

    pub(super) fn show_worktree_path(&mut self, target: String) {
        if self.remote_app_server_url.is_some() {
            self.chat_widget.add_error_message(
                "/worktree is not supported for remote sessions yet.".to_string(),
            );
            return;
        }
        match self.list_current_repo_worktrees() {
            Ok(entries) => match crate::worktree::find_worktree(&entries, &target) {
                Ok(info) => {
                    self.chat_widget.add_info_message(
                        info.workspace_cwd.display().to_string(),
                        /*hint*/ None,
                    );
                }
                Err(err) => self.chat_widget.add_error_message(err),
            },
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to list worktrees: {err}"));
            }
        }
    }

    pub(super) fn remove_worktree(
        &mut self,
        target: String,
        force: bool,
        delete_branch: bool,
        confirmed: bool,
    ) {
        if self.remote_app_server_url.is_some() {
            self.chat_widget.add_error_message(
                "/worktree is not supported for remote sessions yet.".to_string(),
            );
            return;
        }
        let entries = match self.list_current_repo_worktrees() {
            Ok(entries) => entries,
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to list worktrees: {err}"));
                return;
            }
        };
        let info = match crate::worktree::find_worktree(&entries, &target) {
            Ok(info) => info,
            Err(err) => {
                self.chat_widget.add_error_message(err);
                return;
            }
        };
        if info.source != WorktreeSource::Cli {
            let source = crate::worktree::source_label(info.source);
            self.chat_widget.add_error_message(format!(
                "Refusing to remove {source} worktree '{target}'. Only Codex CLI-managed worktrees can be removed."
            ));
            return;
        }
        if !confirmed {
            let params = crate::worktree::remove_confirmation_params(target, force, delete_branch);
            self.chat_widget.show_selection_view(params);
            return;
        }

        match codex_worktree::remove_worktree(WorktreeRemoveRequest {
            codex_home: self.config.codex_home.to_path_buf(),
            source_cwd: Some(self.config.cwd.to_path_buf()),
            name_or_path: target.clone(),
            force,
            delete_branch,
        }) {
            Ok(result) => {
                let mut message = format!("Removed worktree {}", result.removed_path.display());
                if let Some(branch) = result.deleted_branch {
                    message.push_str(&format!(" and deleted branch {branch}"));
                }
                self.chat_widget.add_info_message(message, /*hint*/ None);
            }
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to remove worktree: {err}"));
            }
        }
    }

    fn list_current_repo_worktrees(&self) -> anyhow::Result<Vec<WorktreeInfo>> {
        codex_worktree::list_worktrees(WorktreeListQuery {
            codex_home: self.config.codex_home.to_path_buf(),
            source_cwd: Some(self.config.cwd.to_path_buf()),
            include_all_repos: false,
        })
    }

    fn fetch_worktrees_for_picker(&mut self) {
        let query = WorktreeListQuery {
            codex_home: self.config.codex_home.to_path_buf(),
            source_cwd: Some(self.config.cwd.to_path_buf()),
            include_all_repos: false,
        };
        let cwd = self.config.cwd.to_path_buf();
        let app_event_tx = self.app_event_tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = codex_worktree::list_worktrees(query).map_err(|err| err.to_string());
            app_event_tx.send(AppEvent::WorktreesLoaded { cwd, result });
        });
    }

    async fn switch_to_worktree_info(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        info: WorktreeInfo,
    ) {
        let mut config = match self
            .rebuild_config_for_cwd(info.workspace_cwd.clone())
            .await
        {
            Ok(config) => config,
            Err(err) => {
                self.show_worktree_error(
                    "Failed to rebuild configuration for worktree.".to_string(),
                    err.to_string(),
                );
                return;
            }
        };
        self.apply_runtime_policy_overrides(&mut config);

        if let Some(thread_id) = self.chat_widget.thread_id() {
            match app_server.fork_thread(config.clone(), thread_id).await {
                Ok(forked) => {
                    self.shutdown_current_thread(app_server).await;
                    self.install_worktree_config(tui, config);
                    if let Err(err) = self
                        .replace_chat_widget_with_app_server_thread(
                            tui, app_server, forked, /*initial_user_message*/ None,
                        )
                        .await
                    {
                        self.show_worktree_error(
                            "Failed to attach to worktree thread.".to_string(),
                            err.to_string(),
                        );
                    }
                }
                Err(err) => {
                    self.show_worktree_error(
                        "Failed to fork current session into worktree.".to_string(),
                        err.to_string(),
                    );
                }
            }
        } else {
            self.shutdown_current_thread(app_server).await;
            self.install_worktree_config(tui, config.clone());
            match app_server.start_thread(&config).await {
                Ok(started) => {
                    if let Err(err) = self
                        .replace_chat_widget_with_app_server_thread(
                            tui, app_server, started, /*initial_user_message*/ None,
                        )
                        .await
                    {
                        self.show_worktree_error(
                            "Failed to attach to worktree thread.".to_string(),
                            err.to_string(),
                        );
                    }
                }
                Err(err) => {
                    self.show_worktree_error(
                        "Failed to start session in worktree.".to_string(),
                        err.to_string(),
                    );
                }
            }
        }
        tui.frame_requester().schedule_frame();
    }

    async fn show_worktree_switching_view(&mut self, tui: &mut tui::Tui, target: String) {
        self.chat_widget
            .show_selection_view(crate::worktree::switching_params(
                target,
                tui.frame_requester(),
                self.config.animations,
            ));
        tui.frame_requester().schedule_frame();
        tokio::task::yield_now().await;
    }

    fn replace_worktree_view(&mut self, params: crate::bottom_pane::SelectionViewParams) -> bool {
        self.chat_widget
            .replace_selection_view_if_active(crate::worktree::WORKTREE_SELECTION_VIEW_ID, params)
    }

    fn show_worktree_error(&mut self, summary: String, error: String) {
        let params = crate::worktree::error_with_summary_params(summary.clone(), error.clone());
        if !self.replace_worktree_view(params) {
            self.chat_widget
                .add_error_message(format!("{summary} {error}"));
        }
    }

    fn install_worktree_config(&mut self, tui: &mut tui::Tui, config: Config) {
        self.config = config;
        tui.set_notification_settings(
            self.config.tui_notifications.method,
            self.config.tui_notifications.condition,
        );
        self.file_search
            .update_search_dir(self.config.cwd.to_path_buf());
    }
}
