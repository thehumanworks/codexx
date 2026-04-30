use std::fs;
use std::fs::Metadata;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    dev: u64,
    ino: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SyntheticMountTargetKind {
    EmptyFile,
    EmptyDirectory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyntheticMountTarget {
    path: PathBuf,
    kind: SyntheticMountTargetKind,
    // If an empty metadata path was already present, remember its inode so
    // cleanup does not delete a real pre-existing file or directory.
    pre_existing_path: Option<FileIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProtectedCreateTarget {
    path: PathBuf,
}

impl ProtectedCreateTarget {
    pub(crate) fn missing(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl SyntheticMountTarget {
    pub(crate) fn missing(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: SyntheticMountTargetKind::EmptyFile,
            pre_existing_path: None,
        }
    }

    pub(crate) fn missing_empty_directory(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: SyntheticMountTargetKind::EmptyDirectory,
            pre_existing_path: None,
        }
    }

    pub(crate) fn existing_empty_file(path: &Path, metadata: &Metadata) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: SyntheticMountTargetKind::EmptyFile,
            pre_existing_path: Some(FileIdentity::from_metadata(metadata)),
        }
    }

    pub(crate) fn existing_empty_directory(path: &Path, metadata: &Metadata) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: SyntheticMountTargetKind::EmptyDirectory,
            pre_existing_path: Some(FileIdentity::from_metadata(metadata)),
        }
    }

    pub(crate) fn preserves_pre_existing_path(&self) -> bool {
        self.pre_existing_path.is_some()
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn kind(&self) -> SyntheticMountTargetKind {
        self.kind
    }

    pub(crate) fn should_remove_after_bwrap(&self, metadata: &Metadata) -> bool {
        match self.kind {
            SyntheticMountTargetKind::EmptyFile => {
                if !metadata.file_type().is_file() || metadata.len() != 0 {
                    return false;
                }
            }
            SyntheticMountTargetKind::EmptyDirectory => {
                if !metadata.file_type().is_dir() {
                    return false;
                }
            }
        }

        match self.pre_existing_path {
            Some(pre_existing_path) => pre_existing_path != FileIdentity::from_metadata(metadata),
            None => true,
        }
    }
}

pub(crate) fn should_leave_missing_git_for_parent_repo_discovery(
    mount_root: &Path,
    name: &str,
) -> bool {
    let path = mount_root.join(name);
    name == ".git"
        && matches!(
            path.symlink_metadata(),
            Err(err) if err.kind() == io::ErrorKind::NotFound
        )
        && mount_root
            .ancestors()
            .skip(1)
            .any(ancestor_has_git_metadata)
}

fn ancestor_has_git_metadata(ancestor: &Path) -> bool {
    let git_path = ancestor.join(".git");
    let Ok(metadata) = git_path.symlink_metadata() else {
        return false;
    };
    if metadata.is_dir() {
        return git_path.join("HEAD").symlink_metadata().is_ok();
    }
    if metadata.is_file() {
        return fs::read_to_string(git_path)
            .is_ok_and(|contents| contents.trim_start().starts_with("gitdir:"));
    }
    false
}
