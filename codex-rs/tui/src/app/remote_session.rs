//! Modal-backed remote app-server session startup for `/modal`.

use super::*;
use crate::remote_session::RemoteSandboxSession;
use crate::remote_session::RemoteSessionEndpoint;
use crate::remote_session::RemoteSessionRequest;
use crate::remote_session::RemoteWorkspaceMode;
use ignore::DirEntry;
use ignore::WalkBuilder;
use ignore::WalkState;
use modal_rs::AppOptions;
use modal_rs::ModalClient;
use modal_rs::Sandbox;
use modal_rs::SandboxExecOptions;
use modal_rs::SandboxOptions;
use modal_rs::SandboxTunnel;
use modal_rs::agent_images::get_codex_image;
use std::borrow::Cow;
use std::fs;
use std::path::Component;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::time::Instant;
use tokio::time::sleep;

const MODAL_APP_NAME: &str = "codex-remote";
const MODAL_ENVIRONMENT: &str = "main";
const APP_SERVER_PORT: u32 = 4222;
const APP_SERVER_TOKEN_PATH: &str = "/tmp/codex-app-server-token";
const APP_SERVER_PID_PATH: &str = "/tmp/codex-app-server.pid";
const APP_SERVER_LOG_PATH: &str = "/tmp/codex-app-server.log";
const APP_SERVER_CONNECT_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 45);
const APP_SERVER_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(/*millis*/ 500);
const MODAL_TUNNEL_WAIT_SECS: f32 = 2.0;
const SANDBOX_TIMEOUT_SECS: u32 = 24 * 60 * 60;
const SANDBOX_IDLE_TIMEOUT_SECS: u32 = 60 * 60;
const MAX_COPY_FILES: usize = 10_000;
const MAX_COPY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug)]
struct WorkspaceCopyFile {
    relative_path: String,
    data: Vec<u8>,
}

impl App {
    pub(super) fn begin_start_modal_session(
        &mut self,
        tui: &mut tui::Tui,
        request: RemoteSessionRequest,
    ) {
        if self.chat_widget.is_modal_session_start_running() {
            self.chat_widget
                .add_error_message("A Modal session is already starting.".to_string());
            return;
        }

        self.chat_widget.show_modal_session_start_status(&request);
        let app_event_tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let result = start_modal_app_server(&request)
                .await
                .map_err(|err| format!("{err:#}"));
            app_event_tx.send(AppEvent::ModalSessionStarted { request, result });
        });
        tui.frame_requester().schedule_frame();
    }

    pub(super) async fn handle_modal_session_started(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        request: RemoteSessionRequest,
        result: Result<RemoteSessionEndpoint, String>,
    ) {
        match result {
            Ok(endpoint) => {
                if let Err(err) = self
                    .attach_modal_session_endpoint(tui, app_server, request, endpoint)
                    .await
                {
                    self.chat_widget.clear_modal_session_start_status();
                    self.restore_pending_modal_initial_message();
                    self.chat_widget
                        .add_error_message(format!("Failed to start Modal session: {err}"));
                    tui.frame_requester().schedule_frame();
                }
            }
            Err(err) => {
                self.chat_widget.clear_modal_session_start_status();
                self.restore_pending_modal_initial_message();
                self.chat_widget
                    .add_error_message(format!("Failed to start Modal session: {err}"));
                tui.frame_requester().schedule_frame();
            }
        }
    }

    async fn attach_modal_session_endpoint(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        request: RemoteSessionRequest,
        endpoint: RemoteSessionEndpoint,
    ) -> Result<()> {
        self.chat_widget.add_info_message(
            "Attaching to Modal session...".to_string(),
            Some(format!(
                "Remote working directory: {}",
                request.remote_cwd.display()
            )),
        );
        tui.frame_requester().schedule_frame();

        let summary = session_summary(
            self.chat_widget.token_usage(),
            self.chat_widget.thread_id(),
            self.chat_widget.thread_name(),
            self.chat_widget.rollout_path().as_deref(),
        );

        let mut next_app_server = AppServerSession::new(
            connect_remote_app_server_with_retry(
                endpoint.websocket_url.clone(),
                endpoint.auth_token.clone(),
            )
            .await?,
        )
        .with_remote_cwd_override(Some(endpoint.remote_cwd.clone()));

        let bootstrap = next_app_server.bootstrap(&self.config).await?;
        self.model_catalog = Arc::new(ModelCatalog::new(bootstrap.available_models));
        self.feedback_audience = bootstrap.feedback_audience;
        self.chat_widget.update_account_state(
            bootstrap.status_account_display,
            bootstrap.plan_type,
            bootstrap.has_chatgpt_account,
        );

        let started = next_app_server
            .start_thread_with_session_start_source(&self.config, Some(ThreadStartSource::Clear))
            .await
            .wrap_err("failed to start remote app-server thread")?;

        self.shutdown_current_thread(app_server).await;
        for thread_id in self.thread_event_channels.keys().copied() {
            if let Err(err) = app_server.thread_unsubscribe(thread_id).await {
                tracing::warn!("failed to unsubscribe tracked thread {thread_id}: {err}");
            }
        }

        let old_app_server = std::mem::replace(app_server, next_app_server);
        if let Err(err) = old_app_server.shutdown().await {
            tracing::warn!("failed to shut down previous app-server session: {err}");
        }

        self.remote_app_server_url = Some(endpoint.websocket_url.clone());
        self.remote_app_server_auth_token = Some(endpoint.auth_token.clone());
        self.remote_sandbox_session = Some(RemoteSandboxSession {
            provider: request.provider,
            sandbox_id: endpoint.sandbox_id.clone(),
        });
        self.remote_sandbox_exit_prompt_pending = false;
        self.clear_terminal_ui(tui, /*redraw_header*/ false)?;
        self.reset_app_ui_state_after_clear();
        self.reset_thread_event_state();

        let initial_user_message = self.pending_modal_initial_user_message.take();
        self.replace_chat_widget_with_app_server_thread(
            tui,
            app_server,
            started,
            initial_user_message,
        )
        .await
        .wrap_err("failed to attach TUI to remote app-server thread")?;

        self.chat_widget.add_info_message(
            format!("Connected to Modal sandbox {}.", endpoint.sandbox_id),
            Some(format!(
                "App server: {} | cwd: {}",
                endpoint.websocket_url,
                endpoint.remote_cwd.display()
            )),
        );
        if let Some(summary) = summary {
            let mut lines: Vec<Line<'static>> = Vec::new();
            if let Some(usage_line) = summary.usage_line {
                lines.push(usage_line.into());
            }
            if let Some(command) = summary.resume_command {
                let spans = vec![
                    "To continue the previous session, run ".into(),
                    command.cyan(),
                ];
                lines.push(spans.into());
            }
            self.chat_widget.add_plain_history_lines(lines);
        }
        tui.frame_requester().schedule_frame();
        Ok(())
    }

    fn restore_pending_modal_initial_message(&mut self) {
        if let Some(message) = self.pending_modal_initial_user_message.take() {
            self.chat_widget.restore_user_message_to_composer(message);
        }
    }

    pub(super) fn should_prompt_remote_sandbox_termination(&self, mode: ExitMode) -> bool {
        mode == ExitMode::ShutdownFirst
            && self.remote_sandbox_session.is_some()
            && !self.remote_sandbox_exit_prompt_pending
    }

    pub(super) async fn handle_remote_sandbox_exit_decision(
        &mut self,
        app_server: &mut AppServerSession,
        session: RemoteSandboxSession,
        terminate: bool,
        exit_mode: ExitMode,
    ) -> AppRunControl {
        self.remote_sandbox_exit_prompt_pending = false;
        if terminate {
            match terminate_modal_sandbox(&session).await {
                Ok(()) => {
                    if self.remote_sandbox_session.as_ref() == Some(&session) {
                        self.remote_sandbox_session = None;
                    }
                    tracing::info!(
                        provider = session.provider.as_str(),
                        sandbox_id = session.sandbox_id,
                        "terminated remote sandbox on exit"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        provider = session.provider.as_str(),
                        sandbox_id = session.sandbox_id,
                        error = %err,
                        "failed to terminate remote sandbox on exit"
                    );
                }
            }
        }
        self.finish_exit_mode(app_server, exit_mode).await
    }
}

async fn terminate_modal_sandbox(session: &RemoteSandboxSession) -> Result<()> {
    let mut client = ModalClient::connect()
        .await
        .wrap_err("failed to connect to Modal")?;
    let sandbox = client
        .sandboxes()
        .from_id(&session.sandbox_id)
        .await
        .wrap_err_with(|| {
            format!(
                "failed to attach to Modal sandbox `{}` for termination",
                session.sandbox_id
            )
        })?;
    sandbox
        .terminate(&mut client)
        .await
        .wrap_err_with(|| format!("failed to terminate Modal sandbox `{}`", session.sandbox_id))
}

async fn start_modal_app_server(request: &RemoteSessionRequest) -> Result<RemoteSessionEndpoint> {
    let mut client = ModalClient::connect()
        .await
        .wrap_err("failed to connect to Modal")?;
    let sandbox = match request.sandbox_id.as_deref() {
        Some(sandbox_id) => client
            .sandboxes()
            .from_id(sandbox_id)
            .await
            .wrap_err_with(|| format!("failed to attach to Modal sandbox `{sandbox_id}`"))?,
        None => create_modal_sandbox(&mut client, request).await?,
    };

    prepare_remote_workspace(&mut client, &sandbox, request).await?;
    let auth_token = format!("codex-remote-{}", Uuid::new_v4());
    start_codex_app_server(&mut client, &sandbox, &request.remote_cwd, &auth_token).await?;
    let tunnel = wait_for_app_server_tunnel(&mut client, &sandbox).await?;
    Ok(RemoteSessionEndpoint {
        websocket_url: websocket_url_from_tunnel(&tunnel)?,
        auth_token,
        remote_cwd: request.remote_cwd.clone(),
        sandbox_id: sandbox.id().to_string(),
    })
}

async fn create_modal_sandbox(
    client: &mut ModalClient,
    request: &RemoteSessionRequest,
) -> Result<Sandbox> {
    let app = client
        .get_or_create_app(
            MODAL_APP_NAME,
            MODAL_ENVIRONMENT,
            AppOptions::create_if_missing(),
        )
        .await
        .wrap_err("failed to create Modal app")?;
    let image = get_codex_image(client, MODAL_ENVIRONMENT, &app, /*use_latest*/ true)
        .await
        .wrap_err("failed to resolve Modal Codex image")?;
    let options = SandboxOptions::default()
        .with_timeout(SANDBOX_TIMEOUT_SECS)
        .with_idle_timeout(SANDBOX_IDLE_TIMEOUT_SECS)
        .with_workdir(request.remote_cwd.to_string_lossy())
        .with_unencrypted_ports(vec![APP_SERVER_PORT]);

    client
        .sandboxes()
        .create(&app, &image, options)
        .await
        .wrap_err("failed to create Modal sandbox")
}

async fn prepare_remote_workspace(
    client: &mut ModalClient,
    sandbox: &Sandbox,
    request: &RemoteSessionRequest,
) -> Result<()> {
    match request.workspace_mode {
        RemoteWorkspaceMode::UseRemotePath => ensure_remote_directory(client, sandbox, request)
            .await
            .wrap_err("failed to prepare remote working directory"),
        RemoteWorkspaceMode::CopyCwd => {
            copy_workspace_to_sandbox_filesystem(client, sandbox, request)
                .await
                .wrap_err("failed to copy cwd into Modal sandbox filesystem")
        }
        RemoteWorkspaceMode::GitClone => clone_git_repo_to_sandbox(client, sandbox, request)
            .await
            .wrap_err("failed to clone git repo in Modal sandbox"),
    }
}

async fn copy_workspace_to_sandbox_filesystem(
    client: &mut ModalClient,
    sandbox: &Sandbox,
    request: &RemoteSessionRequest,
) -> Result<()> {
    let cwd = request.local_cwd.clone();
    let files = tokio::task::spawn_blocking(move || collect_workspace_files(&cwd))
        .await
        .wrap_err("failed to join workspace file scan")??;
    let mut filesystem = sandbox
        .filesystem(client)
        .await
        .wrap_err("failed to open Modal sandbox filesystem")?;
    let remote_root = request.remote_cwd.to_string_lossy().to_string();
    filesystem
        .mkdir(&remote_root, /*parents*/ true)
        .await
        .wrap_err_with(|| format!("failed to create remote directory `{remote_root}`"))?;
    let mut created_dirs = std::collections::HashSet::new();
    for file in files {
        let remote_path = join_remote_path(&remote_root, &file.relative_path);
        if let Some(parent) = remote_path.rsplit_once('/').map(|(parent, _)| parent)
            && !parent.is_empty()
            && created_dirs.insert(parent.to_string())
        {
            filesystem
                .mkdir(parent, /*parents*/ true)
                .await
                .wrap_err_with(|| format!("failed to create remote directory `{parent}`"))?;
        }
        filesystem
            .write_file(&remote_path, &file.data, /*overwrite*/ true)
            .await
            .wrap_err_with(|| format!("failed to write remote file `{remote_path}`"))?;
    }
    Ok(())
}

async fn clone_git_repo_to_sandbox(
    client: &mut ModalClient,
    sandbox: &Sandbox,
    request: &RemoteSessionRequest,
) -> Result<()> {
    let cwd = request.local_cwd.clone();
    let repo_url = tokio::task::spawn_blocking(move || git_remote_origin_url(&cwd))
        .await
        .wrap_err("failed to join git remote lookup")??;
    let remote_cwd = request.remote_cwd.to_string_lossy();
    let parent = remote_parent(&remote_cwd);
    let script = format!(
        "set -eu\nmkdir -p {parent}\nif [ -e {remote_cwd} ] && [ \"$(ls -A {remote_cwd} 2>/dev/null)\" ]; then\n  echo 'remote path is not empty' >&2\n  exit 1\nfi\ngit clone {repo_url} {remote_cwd}\n",
        parent = shell_quote(&parent),
        remote_cwd = shell_quote(&remote_cwd),
        repo_url = shell_quote(&repo_url),
    );
    let result = sandbox
        .exec(
            client,
            SandboxExecOptions::new(vec!["/bin/sh".to_string(), "-lc".to_string(), script])
                .with_timeout(/*secs*/ 600),
        )
        .await
        .wrap_err("failed to run git clone in Modal sandbox")?;
    if !result.exit_status.is_success() {
        let stderr = result
            .stderr
            .as_deref()
            .map(String::from_utf8_lossy)
            .unwrap_or(Cow::Borrowed(""));
        color_eyre::eyre::bail!("git clone failed in Modal sandbox: {stderr}");
    }
    Ok(())
}

async fn ensure_remote_directory(
    client: &mut ModalClient,
    sandbox: &Sandbox,
    request: &RemoteSessionRequest,
) -> Result<()> {
    let remote_cwd = request.remote_cwd.to_string_lossy();
    let script = format!("mkdir -p {}", shell_quote(&remote_cwd));
    let result = sandbox
        .exec(
            client,
            SandboxExecOptions::new(vec!["/bin/sh".to_string(), "-lc".to_string(), script])
                .with_timeout(/*secs*/ 60),
        )
        .await
        .wrap_err("failed to create remote working directory")?;
    if !result.exit_status.is_success() {
        color_eyre::eyre::bail!("failed to create remote working directory");
    }
    Ok(())
}

async fn start_codex_app_server(
    client: &mut ModalClient,
    sandbox: &Sandbox,
    remote_cwd: &Path,
    auth_token: &str,
) -> Result<()> {
    let remote_cwd = remote_cwd.to_string_lossy();
    let script = app_server_start_script(&remote_cwd, auth_token);
    let result = sandbox
        .exec(
            client,
            SandboxExecOptions::new(vec!["/bin/sh".to_string(), "-lc".to_string(), script])
                .with_timeout(/*secs*/ 30),
        )
        .await
        .wrap_err("failed to start codex app-server in Modal sandbox")?;
    if !result.exit_status.is_success() {
        let stderr = result
            .stderr
            .as_deref()
            .map(String::from_utf8_lossy)
            .unwrap_or(Cow::Borrowed(""));
        color_eyre::eyre::bail!("codex app-server failed to start in Modal sandbox: {stderr}");
    }
    Ok(())
}

fn app_server_start_script(remote_cwd: &str, auth_token: &str) -> String {
    format!(
        "set -eu\nmkdir -p {remote_cwd}\ncd {remote_cwd}\numask 077\nprintf %s {auth_token} > {token_path}\nnohup codex app-server --listen ws://0.0.0.0:{port} --ws-auth capability-token --ws-token-file {token_path} > {log_path} 2>&1 < /dev/null &\npid=$!\nprintf %s \"$pid\" > {pid_path}\nsleep 1\nif ! kill -0 \"$pid\" 2>/dev/null; then\n  cat {log_path} >&2 || true\n  exit 1\nfi\n",
        remote_cwd = shell_quote(remote_cwd),
        auth_token = shell_quote(auth_token),
        token_path = shell_quote(APP_SERVER_TOKEN_PATH),
        pid_path = shell_quote(APP_SERVER_PID_PATH),
        log_path = shell_quote(APP_SERVER_LOG_PATH),
        port = APP_SERVER_PORT,
    )
}

async fn wait_for_app_server_tunnel(
    client: &mut ModalClient,
    sandbox: &Sandbox,
) -> Result<SandboxTunnel> {
    let deadline = Instant::now() + APP_SERVER_CONNECT_TIMEOUT;
    loop {
        let last_error = match sandbox.tunnels(client, MODAL_TUNNEL_WAIT_SECS).await {
            Ok(mut tunnels) => {
                if let Some(tunnel) = tunnels.remove(&APP_SERVER_PORT) {
                    return Ok(tunnel);
                }
                format!("port {APP_SERVER_PORT} was not exposed")
            }
            Err(err) => err.to_string(),
        };
        if Instant::now() >= deadline {
            color_eyre::eyre::bail!("failed to resolve Modal app-server tunnel: {last_error}");
        }
        sleep(APP_SERVER_CONNECT_RETRY_DELAY).await;
    }
}

async fn connect_remote_app_server_with_retry(
    websocket_url: String,
    auth_token: String,
) -> Result<codex_app_server_client::AppServerClient> {
    let deadline = Instant::now() + APP_SERVER_CONNECT_TIMEOUT;
    loop {
        let last_error = match crate::connect_remote_app_server(
            websocket_url.clone(),
            Some(auth_token.clone()),
            /*allow_insecure_auth_token_transport*/ true,
        )
        .await
        {
            Ok(client) => return Ok(client),
            Err(err) => err.to_string(),
        };
        if Instant::now() >= deadline {
            color_eyre::eyre::bail!(
                "failed to connect to remote app-server at `{websocket_url}`: {last_error}"
            );
        }
        sleep(APP_SERVER_CONNECT_RETRY_DELAY).await;
    }
}

fn websocket_url_from_tunnel(tunnel: &SandboxTunnel) -> Result<String> {
    let (Some(host), Some(port)) = (&tunnel.unencrypted_host, tunnel.unencrypted_port) else {
        color_eyre::eyre::bail!(
            "Modal tunnel for port {} did not include an unencrypted endpoint",
            tunnel.container_port
        );
    };
    Ok(format!("ws://{host}:{port}"))
}

fn collect_workspace_files(cwd: &Path) -> Result<Vec<WorkspaceCopyFile>> {
    let relative_paths = gitignore_workspace_paths(cwd)?;
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    for relative_path in relative_paths {
        let absolute_path = cwd.join(&relative_path);
        let Ok(metadata) = fs::metadata(&absolute_path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        total_bytes = total_bytes.saturating_add(metadata.len());
        if files.len() >= MAX_COPY_FILES || total_bytes > MAX_COPY_BYTES {
            color_eyre::eyre::bail!(
                "cwd copy is too large for /modal --copy-cwd (limit: {MAX_COPY_FILES} files or {} MiB)",
                MAX_COPY_BYTES / 1024 / 1024
            );
        }
        let relative_path = upload_path(&relative_path)?;
        let data = fs::read(&absolute_path)
            .wrap_err_with(|| format!("failed to read `{}`", absolute_path.display()))?;
        files.push(WorkspaceCopyFile {
            relative_path,
            data,
        });
    }
    if files.is_empty() {
        color_eyre::eyre::bail!("cwd copy found no files to upload");
    }
    Ok(files)
}

fn gitignore_workspace_paths(cwd: &Path) -> Result<Vec<PathBuf>> {
    let root = Arc::new(cwd.to_path_buf());
    let paths = Arc::new(Mutex::new(Vec::new()));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(1, 8);

    let mut builder = WalkBuilder::new(root.as_path());
    builder
        .hidden(false)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .parents(true)
        .threads(threads);

    builder.build_parallel().run(|| {
        let root = Arc::clone(&root);
        let paths = Arc::clone(&paths);
        let errors = Arc::clone(&errors);
        Box::new(move |entry| match entry {
            Ok(entry) => {
                if should_skip_workspace_entry(&entry) {
                    return WalkState::Skip;
                }
                if entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_file())
                {
                    match entry.path().strip_prefix(root.as_path()) {
                        Ok(path) if !path.as_os_str().is_empty() => {
                            if let Ok(mut paths) = paths.lock() {
                                paths.push(path.to_path_buf());
                            }
                        }
                        Ok(_) => {}
                        Err(err) => {
                            if let Ok(mut errors) = errors.lock() {
                                errors.push(format!(
                                    "failed to relativize `{}`: {err}",
                                    entry.path().display()
                                ));
                            }
                        }
                    }
                }
                WalkState::Continue
            }
            Err(err) => {
                if let Ok(mut errors) = errors.lock() {
                    errors.push(err.to_string());
                }
                WalkState::Continue
            }
        })
    });

    let errors = errors
        .lock()
        .map(|errors| errors.clone())
        .unwrap_or_default();
    if let Some(err) = errors.first() {
        color_eyre::eyre::bail!("failed to walk cwd for /modal --copy-cwd: {err}");
    }

    let mut paths = paths.lock().map(|paths| paths.clone()).unwrap_or_default();
    paths.sort();
    if paths.is_empty() {
        color_eyre::eyre::bail!("cwd copy found no files to upload");
    }
    Ok(paths)
}

fn should_skip_workspace_entry(entry: &DirEntry) -> bool {
    entry.depth() > 0
        && entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        && matches!(
            entry.file_name().to_str(),
            Some(".git" | "target" | ".codex" | ".jj")
        )
}

fn upload_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            _ => {
                color_eyre::eyre::bail!("unsupported workspace path `{}`", path.display());
            }
        }
    }
    Ok(parts.join("/"))
}

fn git_remote_origin_url(cwd: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .wrap_err("failed to run git config")?;
    if !output.status.success() {
        color_eyre::eyre::bail!("current cwd is not a git repo with remote.origin.url");
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        color_eyre::eyre::bail!("current cwd has an empty remote.origin.url");
    }
    Ok(url)
}

fn remote_parent(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|path| path.to_string_lossy().to_string())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "/".to_string())
}

fn join_remote_path(root: &str, relative_path: &str) -> String {
    format!("{}/{}", root.trim_end_matches('/'), relative_path)
}

fn shell_quote(value: &str) -> String {
    shlex::try_quote(value)
        .map(Cow::into_owned)
        .unwrap_or_else(|_| "''".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeSet;
    use tempfile::tempdir;

    #[test]
    fn app_server_start_script_uses_capability_token_auth() {
        let script = app_server_start_script("/workspace/codex", "secret-token");

        assert!(script.contains("printf %s secret-token > /tmp/codex-app-server-token"));
        assert!(script.contains("codex app-server --listen ws://0.0.0.0:4222"));
        assert!(script.contains("--ws-auth capability-token"));
        assert!(script.contains("--ws-token-file /tmp/codex-app-server-token"));
    }

    #[test]
    fn websocket_url_uses_unencrypted_tunnel_endpoint() -> Result<()> {
        let tunnel = SandboxTunnel {
            container_port: APP_SERVER_PORT,
            host: "example.modal.run".to_string(),
            port: 443,
            unencrypted_host: Some("tcp.example.modal.run".to_string()),
            unencrypted_port: Some(42_222),
        };

        assert_eq!(
            websocket_url_from_tunnel(&tunnel)?,
            "ws://tcp.example.modal.run:42222"
        );
        Ok(())
    }

    #[test]
    fn websocket_url_requires_unencrypted_tunnel_endpoint() {
        let tunnel = SandboxTunnel {
            container_port: APP_SERVER_PORT,
            host: "example.modal.run".to_string(),
            port: 443,
            unencrypted_host: None,
            unencrypted_port: None,
        };

        let err = websocket_url_from_tunnel(&tunnel).expect_err("unencrypted endpoint is required");
        assert!(
            err.to_string()
                .contains("did not include an unencrypted endpoint")
        );
    }

    #[test]
    fn collect_workspace_files_respects_gitignore_patterns() -> Result<()> {
        let temp = tempdir()?;
        fs::write(
            temp.path().join(".gitignore"),
            "ignored.txt\nignored-dir/\n",
        )?;
        fs::write(temp.path().join("kept.txt"), "kept")?;
        fs::write(temp.path().join(".hidden"), "hidden")?;
        fs::write(temp.path().join("ignored.txt"), "ignored")?;
        fs::create_dir(temp.path().join("ignored-dir"))?;
        fs::write(temp.path().join("ignored-dir/file.txt"), "ignored")?;
        fs::create_dir(temp.path().join("nested"))?;
        fs::write(temp.path().join("nested/kept.md"), "nested")?;

        let paths = collect_workspace_files(temp.path())?
            .into_iter()
            .map(|file| file.relative_path)
            .collect::<BTreeSet<_>>();

        assert_eq!(
            paths,
            BTreeSet::from([
                ".gitignore".to_string(),
                ".hidden".to_string(),
                "kept.txt".to_string(),
                "nested/kept.md".to_string()
            ])
        );
        Ok(())
    }

    #[tokio::test]
    async fn remote_sandbox_exit_prompt_gate_requires_shutdown_and_session() {
        let mut app = super::super::test_support::make_test_app().await;

        assert!(!app.should_prompt_remote_sandbox_termination(ExitMode::ShutdownFirst));

        app.remote_sandbox_session = Some(RemoteSandboxSession {
            provider: crate::remote_session::RemoteProvider::Modal,
            sandbox_id: "sb-test".to_string(),
        });
        assert!(app.should_prompt_remote_sandbox_termination(ExitMode::ShutdownFirst));
        assert!(!app.should_prompt_remote_sandbox_termination(ExitMode::Immediate));

        app.remote_sandbox_exit_prompt_pending = true;
        assert!(!app.should_prompt_remote_sandbox_termination(ExitMode::ShutdownFirst));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn modal_live_remote_session_branches_clean_up_resources() -> Result<()> {
        if std::env::var_os("CODEX_TUI_RUN_MODAL_INTEGRATION").is_none() {
            return Ok(());
        }

        let mut cleanup = Vec::new();
        let result = run_modal_live_remote_session_branches(&mut cleanup).await;
        cleanup_modal_sandboxes(cleanup).await;
        result
    }

    async fn run_modal_live_remote_session_branches(cleanup: &mut Vec<String>) -> Result<()> {
        let local_cwd = std::env::current_dir()?.join("..");

        let simple_request = RemoteSessionRequest {
            provider: crate::remote_session::RemoteProvider::Modal,
            sandbox_id: None,
            remote_cwd: unique_remote_path("simple"),
            workspace_mode: RemoteWorkspaceMode::UseRemotePath,
            local_cwd: local_cwd.clone(),
        };
        let simple = start_modal_app_server(&simple_request).await?;
        cleanup.push(simple.sandbox_id.clone());
        assert_endpoint_connects(&simple).await?;

        let mut client = ModalClient::connect().await?;
        let reuse_request = RemoteSessionRequest {
            provider: crate::remote_session::RemoteProvider::Modal,
            sandbox_id: None,
            remote_cwd: unique_remote_path("reuse"),
            workspace_mode: RemoteWorkspaceMode::UseRemotePath,
            local_cwd: local_cwd.clone(),
        };
        let reuse_seed = create_modal_sandbox(&mut client, &reuse_request).await?;
        let reuse_sandbox_id = reuse_seed.id().to_string();
        cleanup.push(reuse_sandbox_id.clone());
        let reuse = start_modal_app_server(&RemoteSessionRequest {
            sandbox_id: Some(reuse_sandbox_id),
            ..reuse_request
        })
        .await?;
        assert_endpoint_connects(&reuse).await?;

        let copy_temp = tempdir()?;
        fs::write(copy_temp.path().join(".gitignore"), "ignored.txt\n")?;
        fs::write(copy_temp.path().join("kept.txt"), "copied")?;
        fs::write(copy_temp.path().join("ignored.txt"), "ignored")?;
        let copy_request = RemoteSessionRequest {
            provider: crate::remote_session::RemoteProvider::Modal,
            sandbox_id: None,
            remote_cwd: unique_remote_path("copy"),
            workspace_mode: RemoteWorkspaceMode::CopyCwd,
            local_cwd: copy_temp.path().to_path_buf(),
        };
        let copy = start_modal_app_server(&copy_request).await?;
        cleanup.push(copy.sandbox_id.clone());
        assert_endpoint_connects(&copy).await?;
        let copy_check = exec_shell(
            &mut client,
            &copy.sandbox_id,
            &format!(
                "test -f {kept} && test ! -e {ignored} && printf copy-ok",
                kept = shell_quote(&join_remote_path(
                    &copy.remote_cwd.to_string_lossy(),
                    "kept.txt"
                )),
                ignored = shell_quote(&join_remote_path(
                    &copy.remote_cwd.to_string_lossy(),
                    "ignored.txt"
                )),
            ),
        )
        .await?;
        assert_eq!(copy_check, "copy-ok");

        let git_request = RemoteSessionRequest {
            provider: crate::remote_session::RemoteProvider::Modal,
            sandbox_id: None,
            remote_cwd: unique_remote_path("git"),
            workspace_mode: RemoteWorkspaceMode::GitClone,
            local_cwd,
        };
        let git = start_modal_app_server(&git_request).await?;
        cleanup.push(git.sandbox_id.clone());
        assert_endpoint_connects(&git).await?;
        let git_check = exec_shell(
            &mut client,
            &git.sandbox_id,
            &format!(
                "test -f {cargo_toml} && printf git-ok",
                cargo_toml = shell_quote(&join_remote_path(
                    &git.remote_cwd.to_string_lossy(),
                    "codex-rs/Cargo.toml"
                )),
            ),
        )
        .await?;
        assert_eq!(git_check, "git-ok");

        let keep_request = RemoteSessionRequest {
            provider: crate::remote_session::RemoteProvider::Modal,
            sandbox_id: None,
            remote_cwd: unique_remote_path("keep"),
            workspace_mode: RemoteWorkspaceMode::UseRemotePath,
            local_cwd: std::env::current_dir()?,
        };
        let keep_sandbox = create_modal_sandbox(&mut client, &keep_request).await?;
        let keep_session = RemoteSandboxSession {
            provider: crate::remote_session::RemoteProvider::Modal,
            sandbox_id: keep_sandbox.id().to_string(),
        };
        cleanup.push(keep_session.sandbox_id.clone());
        let still_running =
            exec_shell(&mut client, &keep_session.sandbox_id, "printf keep-running").await?;
        assert_eq!(still_running, "keep-running");
        terminate_modal_sandbox(&keep_session).await?;
        cleanup.retain(|sandbox_id| sandbox_id != &keep_session.sandbox_id);
        wait_for_sandbox_to_stop(&mut client, &keep_session.sandbox_id).await?;

        Ok(())
    }

    async fn assert_endpoint_connects(endpoint: &RemoteSessionEndpoint) -> Result<()> {
        let client = connect_remote_app_server_with_retry(
            endpoint.websocket_url.clone(),
            endpoint.auth_token.clone(),
        )
        .await?;
        drop(client);
        Ok(())
    }

    async fn cleanup_modal_sandboxes(sandbox_ids: Vec<String>) {
        for sandbox_id in sandbox_ids {
            let session = RemoteSandboxSession {
                provider: crate::remote_session::RemoteProvider::Modal,
                sandbox_id,
            };
            let _ = terminate_modal_sandbox(&session).await;
        }
    }

    async fn exec_shell(
        client: &mut ModalClient,
        sandbox_id: &str,
        script: &str,
    ) -> Result<String> {
        let sandbox = client.sandboxes().from_id(sandbox_id).await?;
        let result = sandbox
            .exec(
                client,
                SandboxExecOptions::new(vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    script.to_string(),
                ])
                .with_timeout(/*secs*/ 60),
            )
            .await?;
        if !result.exit_status.is_success() {
            let stderr = result
                .stderr
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or(Cow::Borrowed(""));
            color_eyre::eyre::bail!("sandbox command failed: {stderr}");
        }
        Ok(result
            .stdout
            .as_deref()
            .map(String::from_utf8_lossy)
            .unwrap_or(Cow::Borrowed(""))
            .into_owned())
    }

    async fn wait_for_sandbox_to_stop(client: &mut ModalClient, sandbox_id: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(/*secs*/ 30);
        loop {
            if exec_shell(client, sandbox_id, "true").await.is_err() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                color_eyre::eyre::bail!("Modal sandbox `{sandbox_id}` was still running");
            }
            sleep(Duration::from_secs(/*secs*/ 1)).await;
        }
    }

    fn unique_remote_path(label: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/codex-remote-{label}-{}", Uuid::new_v4()))
    }
}
