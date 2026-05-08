use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;

use crate::git;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyPolicy {
    Fail,
    Ignore,
    CopyTracked,
    CopyAll,
    MoveAll,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirtyState {
    pub has_staged_changes: bool,
    pub has_unstaged_changes: bool,
    pub has_untracked_files: bool,
}

impl DirtyState {
    pub fn is_dirty(&self) -> bool {
        self.has_staged_changes || self.has_unstaged_changes || self.has_untracked_files
    }
}

pub fn dirty_state(root: &Path) -> Result<DirtyState> {
    let staged = git::bytes(root, &["diff", "--cached", "--name-only", "-z"])?;
    let unstaged = git::bytes(root, &["diff", "--name-only", "-z"])?;
    let untracked = git::bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    Ok(DirtyState {
        has_staged_changes: !staged.is_empty(),
        has_unstaged_changes: !unstaged.is_empty(),
        has_untracked_files: !untracked.is_empty(),
    })
}

pub fn validate_dirty_policy_before_create(
    source_root: &Path,
    policy: DirtyPolicy,
) -> Result<Vec<String>> {
    let state = dirty_state(source_root)?;
    if !state.is_dirty() {
        return Ok(Vec::new());
    }

    match policy {
        DirtyPolicy::Fail => bail_for_dirty_source(),
        DirtyPolicy::Ignore => Ok(vec![
            "source checkout has uncommitted changes; the new worktree was created without them"
                .to_string(),
        ]),
        DirtyPolicy::CopyTracked => {
            if state.has_untracked_files {
                Ok(vec![
                    "untracked files were left in the source checkout; use --worktree-dirty copy-all or move-all to carry them"
                        .to_string(),
                ])
            } else {
                Ok(Vec::new())
            }
        }
        DirtyPolicy::CopyAll | DirtyPolicy::MoveAll => Ok(Vec::new()),
    }
}

pub fn apply_dirty_policy_after_create(
    source_root: &Path,
    worktree_root: &Path,
    policy: DirtyPolicy,
) -> Result<()> {
    let state = dirty_state(source_root)?;
    if !state.is_dirty() {
        return Ok(());
    }

    match policy {
        DirtyPolicy::Fail | DirtyPolicy::Ignore => Ok(()),
        DirtyPolicy::CopyTracked => apply_tracked_diff(source_root, worktree_root),
        DirtyPolicy::CopyAll => {
            apply_tracked_diff(source_root, worktree_root)?;
            copy_untracked_files(source_root, worktree_root)?;
            Ok(())
        }
        DirtyPolicy::MoveAll => {
            let untracked_paths = untracked_paths(source_root)?;
            apply_tracked_diff(source_root, worktree_root)?;
            copy_untracked_files_at_paths(source_root, worktree_root, &untracked_paths)?;
            clean_source_after_move(source_root, &untracked_paths)?;
            Ok(())
        }
    }
}

fn bail_for_dirty_source<T>() -> Result<T> {
    anyhow::bail!(
        "source checkout has uncommitted changes; use --worktree-dirty ignore, copy-tracked, copy-all, or move-all"
    );
}

fn apply_tracked_diff(source_root: &Path, worktree_root: &Path) -> Result<()> {
    let staged = git::bytes(source_root, &["diff", "--cached", "--binary"])?;
    let unstaged = git::bytes(source_root, &["diff", "--binary"])?;

    if !staged.is_empty() {
        git::status_with_stdin(
            worktree_root,
            &["apply", "--index", "--binary", "-"],
            &staged,
        )
        .context("failed to apply staged changes to worktree")?;
    }
    if !unstaged.is_empty() {
        git::status_with_stdin(worktree_root, &["apply", "--binary", "-"], &unstaged)
            .context("failed to apply unstaged changes to worktree")?;
    }
    Ok(())
}

fn copy_untracked_files(source_root: &Path, worktree_root: &Path) -> Result<()> {
    let paths = untracked_paths(source_root)?;
    copy_untracked_files_at_paths(source_root, worktree_root, &paths)
}

fn untracked_paths(source_root: &Path) -> Result<Vec<PathBuf>> {
    let output = git::bytes(
        source_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|raw_path| {
            let relative_path = PathBuf::from(String::from_utf8_lossy(raw_path).into_owned());
            ensure_safe_relative_path(&relative_path)?;
            Ok(relative_path)
        })
        .collect()
}

fn copy_untracked_files_at_paths(
    source_root: &Path,
    worktree_root: &Path,
    paths: &[PathBuf],
) -> Result<()> {
    for relative_path in paths {
        ensure_safe_relative_path(relative_path)?;
        let source = source_root.join(relative_path);
        let destination = worktree_root.join(relative_path);
        let metadata = fs::symlink_metadata(&source)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&source)?;
            create_symlink(&target, &destination)?;
        } else if metadata.is_file() {
            fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

fn clean_source_after_move(source_root: &Path, untracked_paths: &[PathBuf]) -> Result<()> {
    git::status(source_root, &["reset", "--hard", "HEAD"])
        .context("failed to clean tracked changes from source checkout after move")?;
    for relative_path in untracked_paths {
        fs::remove_file(source_root.join(relative_path)).with_context(|| {
            format!(
                "failed to remove moved untracked path {} from source checkout",
                relative_path.display()
            )
        })?;
    }
    Ok(())
}

fn ensure_safe_relative_path(path: &Path) -> Result<()> {
    if path.is_absolute() {
        anyhow::bail!(
            "refusing to copy absolute untracked path {}",
            path.display()
        );
    }
    if path.components().any(|component| {
        matches!(component, Component::ParentDir)
            || matches!(component, Component::Normal(value) if value == ".git")
    }) {
        anyhow::bail!("refusing to copy unsafe untracked path {}", path.display());
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, destination).map_err(Into::into)
}

#[cfg(windows)]
fn create_symlink(target: &Path, destination: &Path) -> Result<()> {
    std::os::windows::fs::symlink_file(target, destination).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_state_reports_clean_by_default() {
        assert!(!DirtyState::default().is_dirty());
    }
}
