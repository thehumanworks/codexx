use std::fs;
use std::path::Path;
use std::process::Command;

use codex_worktree::DirtyPolicy;
use codex_worktree::WorktreeListQuery;
use codex_worktree::WorktreeLocation;
use codex_worktree::WorktreeRemoveRequest;
use codex_worktree::WorktreeRequest;
use codex_worktree::WorktreeSource;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[test]
fn creates_reuses_lists_and_removes_managed_worktree() -> anyhow::Result<()> {
    let fixture = GitFixture::new()?;
    fs::create_dir(fixture.repo.path().join("codex-rs"))?;
    fs::write(fixture.repo.path().join("codex-rs/README.md"), "subdir\n")?;
    run_git(fixture.repo.path(), &["add", "codex-rs/README.md"])?;
    run_git(fixture.repo.path(), &["commit", "-m", "add subdir"])?;

    let resolution = codex_worktree::ensure_worktree(WorktreeRequest {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: fixture.repo.path().join("codex-rs"),
        branch: "parser-fix".to_string(),
        base_ref: None,
        dirty_policy: DirtyPolicy::Fail,
    })?;

    assert!(!resolution.reused);
    assert_eq!(resolution.info.name, "parser-fix");
    assert_eq!(resolution.info.slug, "parser-fix");
    assert_eq!(resolution.info.branch.as_deref(), Some("parser-fix"));
    assert_eq!(resolution.info.source, WorktreeSource::Cli);
    assert_eq!(resolution.info.location, WorktreeLocation::Sibling);
    let canonical_repo = fixture.repo.path().canonicalize()?;
    assert_eq!(
        resolution.info.worktree_git_root,
        canonical_repo.with_file_name(format!(
            "{}.parser-fix",
            canonical_repo.file_name().unwrap().to_string_lossy()
        ))
    );
    assert_eq!(
        resolution.info.workspace_cwd,
        resolution.info.worktree_git_root.join("codex-rs")
    );
    assert!(resolution.info.workspace_cwd.exists());
    assert!(
        git(
            &resolution.info.worktree_git_root,
            &["rev-parse", "--git-path", "codex-worktree.json"]
        )
        .map(|path| resolution.info.worktree_git_root.join(path).exists())
        .unwrap_or(false)
    );

    let reused = codex_worktree::ensure_worktree(WorktreeRequest {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: fixture.repo.path().join("codex-rs"),
        branch: "parser-fix".to_string(),
        base_ref: None,
        dirty_policy: DirtyPolicy::Fail,
    })?;
    assert!(reused.reused);
    assert_eq!(
        reused.info.worktree_git_root,
        resolution.info.worktree_git_root
    );

    let entries = codex_worktree::list_worktrees(WorktreeListQuery {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: Some(fixture.repo.path().to_path_buf()),
        include_all_repos: false,
    })?;
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.source == WorktreeSource::Cli)
            .map(|entry| entry.branch.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("parser-fix")]
    );

    let removed = codex_worktree::remove_worktree(WorktreeRemoveRequest {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: Some(fixture.repo.path().to_path_buf()),
        name_or_path: "parser-fix".to_string(),
        force: false,
        delete_branch: false,
    })?;
    assert_eq!(removed.removed_path, resolution.info.worktree_git_root);
    assert!(!removed.removed_path.exists());
    Ok(())
}

#[test]
fn creates_sibling_from_sibling_using_primary_repo_name() -> anyhow::Result<()> {
    let fixture = GitFixture::new()?;
    let first = codex_worktree::ensure_worktree(WorktreeRequest {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: fixture.repo.path().to_path_buf(),
        branch: "fcoury/worktrees".to_string(),
        base_ref: None,
        dirty_policy: DirtyPolicy::Fail,
    })?;

    let second = codex_worktree::ensure_worktree(WorktreeRequest {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: first.info.workspace_cwd,
        branch: "fcoury/test".to_string(),
        base_ref: None,
        dirty_policy: DirtyPolicy::Fail,
    })?;

    let canonical_repo = fixture.repo.path().canonicalize()?;
    assert_eq!(
        second.info.worktree_git_root,
        canonical_repo.with_file_name(format!(
            "{}.fcoury-test",
            canonical_repo.file_name().unwrap().to_string_lossy()
        ))
    );
    Ok(())
}

#[test]
fn copy_tracked_preserves_staged_and_unstaged_diffs() -> anyhow::Result<()> {
    let fixture = GitFixture::new()?;
    fs::write(fixture.repo.path().join("staged.txt"), "staged changed\n")?;
    run_git(fixture.repo.path(), &["add", "staged.txt"])?;
    fs::write(
        fixture.repo.path().join("unstaged.txt"),
        "unstaged changed\n",
    )?;

    let resolution = codex_worktree::ensure_worktree(WorktreeRequest {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: fixture.repo.path().to_path_buf(),
        branch: "copy-dirty".to_string(),
        base_ref: None,
        dirty_policy: DirtyPolicy::CopyTracked,
    })?;

    assert_eq!(
        git(
            &resolution.info.worktree_git_root,
            &["diff", "--cached", "--name-only"]
        )?,
        "staged.txt"
    );
    assert_eq!(
        git(&resolution.info.worktree_git_root, &["diff", "--name-only"])?,
        "unstaged.txt"
    );
    Ok(())
}

#[test]
fn refuses_sibling_path_collision_for_different_branch() -> anyhow::Result<()> {
    let fixture = GitFixture::new()?;
    let resolution = codex_worktree::ensure_worktree(WorktreeRequest {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: fixture.repo.path().to_path_buf(),
        branch: "feature/foo".to_string(),
        base_ref: None,
        dirty_policy: DirtyPolicy::Fail,
    })?;

    let err = codex_worktree::ensure_worktree(WorktreeRequest {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: fixture.repo.path().to_path_buf(),
        branch: "feature-foo".to_string(),
        base_ref: None,
        dirty_policy: DirtyPolicy::Fail,
    })
    .expect_err("sanitized branch path collision should fail");

    assert!(
        err.to_string().contains("already used by feature/foo"),
        "{err:#}"
    );
    let removed = codex_worktree::remove_worktree(WorktreeRemoveRequest {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: Some(fixture.repo.path().to_path_buf()),
        name_or_path: "feature/foo".to_string(),
        force: false,
        delete_branch: false,
    })?;
    assert_eq!(removed.removed_path, resolution.info.worktree_git_root);
    Ok(())
}

#[test]
fn list_includes_app_style_worktrees_without_cli_metadata() -> anyhow::Result<()> {
    let fixture = GitFixture::new()?;
    let app_worktree = fixture.codex_home.path().join("worktrees/7f6e/repo");
    fs::create_dir_all(app_worktree.parent().expect("app worktree parent"))?;
    run_git(
        fixture.repo.path(),
        &[
            "worktree",
            "add",
            app_worktree.to_str().expect("utf-8 path"),
            "HEAD",
        ],
    )?;

    let entries = codex_worktree::list_worktrees(WorktreeListQuery {
        codex_home: fixture.codex_home.path().to_path_buf(),
        source_cwd: Some(fixture.repo.path().to_path_buf()),
        include_all_repos: false,
    })?;

    let canonical_app_worktree = app_worktree.canonicalize()?;
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.source == WorktreeSource::App)
            .map(|entry| (entry.name.as_str(), entry.worktree_git_root.as_path()))
            .collect::<Vec<_>>(),
        vec![("repo", canonical_app_worktree.as_path())]
    );
    Ok(())
}

struct GitFixture {
    repo: TempDir,
    codex_home: TempDir,
}

impl GitFixture {
    fn new() -> anyhow::Result<Self> {
        let repo = TempDir::new()?;
        let codex_home = TempDir::new()?;
        run_git(repo.path(), &["init", "-b", "main"])?;
        run_git(repo.path(), &["config", "user.email", "codex@example.com"])?;
        run_git(repo.path(), &["config", "user.name", "Codex"])?;
        fs::write(repo.path().join("staged.txt"), "staged original\n")?;
        fs::write(repo.path().join("unstaged.txt"), "unstaged original\n")?;
        run_git(repo.path(), &["add", "."])?;
        run_git(repo.path(), &["commit", "-m", "initial"])?;
        Ok(Self { repo, codex_home })
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim_end().to_string())
}
