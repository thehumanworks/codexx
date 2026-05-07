use anyhow::Context as _;
use std::path::Path;
use std::path::PathBuf;
use tokio::process::Command;

pub async fn run_linux_app_open_or_install(
    workspace: PathBuf,
    download_url_override: Option<String>,
) -> anyhow::Result<()> {
    if let Some(desktop_file) = find_existing_codex_desktop_file() {
        eprintln!("Opening Codex Desktop...");
        open_codex_app(&desktop_file, &workspace).await?;
        return Ok(());
    }

    if let Some(download_url) = download_url_override {
        eprintln!("Codex Desktop not found; opening Linux installer...");
        open_url(&download_url).await?;
        eprintln!(
            "After installing Codex Desktop, rerun `codex app {workspace}`.",
            workspace = workspace.display()
        );
        return Ok(());
    }

    anyhow::bail!(
        "Codex Desktop is not installed. Install the Linux desktop package, then rerun `codex app`."
    );
}

fn find_existing_codex_desktop_file() -> Option<PathBuf> {
    candidate_desktop_file_dirs()
        .into_iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| is_codex_desktop_file(path))
}

fn candidate_desktop_file_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(xdg_data_home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(xdg_data_home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("applications"),
        );
    }

    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| PathBuf::from(dir).join("applications")),
    );
    dirs
}

fn is_codex_desktop_file(path: &Path) -> bool {
    if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
        return false;
    }
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    desktop_file_declares_codex(&contents)
}

fn desktop_file_declares_codex(contents: &str) -> bool {
    contents.lines().any(|line| line.trim() == "Name=Codex")
}

async fn open_codex_app(desktop_file: &Path, workspace: &Path) -> anyhow::Result<()> {
    eprintln!(
        "Opening workspace {workspace}...",
        workspace = workspace.display()
    );
    let status = Command::new("gio")
        .arg("launch")
        .arg(desktop_file)
        .arg(workspace)
        .status()
        .await
        .context("failed to invoke `gio launch`")?;

    if status.success() {
        return Ok(());
    }

    anyhow::bail!(
        "`gio launch {desktop_file} {workspace}` exited with {status}",
        desktop_file = desktop_file.display(),
        workspace = workspace.display()
    );
}

async fn open_url(url: &str) -> anyhow::Result<()> {
    let status = Command::new("xdg-open")
        .arg(url)
        .status()
        .await
        .with_context(|| format!("failed to open {url}"))?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("failed to open {url} with {status}");
    }
}

#[cfg(test)]
mod tests {
    use super::desktop_file_declares_codex;

    #[test]
    fn recognizes_codex_desktop_file_name() {
        assert!(desktop_file_declares_codex(
            "[Desktop Entry]\nName=Codex\nExec=codex %U\n"
        ));
    }

    #[test]
    fn ignores_other_desktop_file_names() {
        assert!(!desktop_file_declares_codex(
            "[Desktop Entry]\nName=Codex Nightly\nExec=codex-nightly %U\n"
        ));
    }
}
