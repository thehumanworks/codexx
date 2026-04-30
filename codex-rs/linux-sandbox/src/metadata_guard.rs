use std::fs;
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use crate::metadata_guard_watcher::ProtectedCreateWatcher;
use crate::metadata_paths::ProtectedCreateTarget;
use crate::metadata_paths::SyntheticMountTarget;
use crate::metadata_paths::SyntheticMountTargetKind;

const METADATA_GUARD_MARKER_SYNTHETIC: &[u8] = b"synthetic\n";
const METADATA_GUARD_MARKER_EXISTING: &[u8] = b"existing\n";
const PROTECTED_CREATE_MARKER: &[u8] = b"protected-create\n";

#[derive(Debug)]
struct SyntheticMountTargetRegistration {
    target: SyntheticMountTarget,
    marker_file: PathBuf,
    marker_dir: PathBuf,
}

#[derive(Debug)]
struct ProtectedCreateTargetRegistration {
    target: ProtectedCreateTarget,
    marker_file: PathBuf,
    marker_dir: PathBuf,
}

pub(crate) struct MetadataGuardRegistrations {
    synthetic_mounts: Vec<SyntheticMountTargetRegistration>,
    protected_creates: Vec<ProtectedCreateTargetRegistration>,
}

pub(crate) struct ProtectedCreateMonitor {
    stop: Arc<AtomicBool>,
    violation: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProtectedCreateRemoval {
    Directory,
    Other,
}

impl MetadataGuardRegistrations {
    pub(crate) fn register(
        synthetic_mounts: &[SyntheticMountTarget],
        protected_creates: &[ProtectedCreateTarget],
    ) -> Self {
        Self {
            synthetic_mounts: register_synthetic_mount_targets(synthetic_mounts),
            protected_creates: register_protected_create_targets(protected_creates),
        }
    }

    pub(crate) fn cleanup(self, protected_create_monitor_violation: bool) -> bool {
        cleanup_synthetic_mount_targets(&self.synthetic_mounts);
        protected_create_monitor_violation
            || cleanup_protected_create_targets(&self.protected_creates)
    }
}

impl ProtectedCreateMonitor {
    pub(crate) fn start(targets: &[ProtectedCreateTarget]) -> Option<Self> {
        if targets.is_empty() {
            return None;
        }

        let targets = targets.to_vec();
        let stop = Arc::new(AtomicBool::new(false));
        let violation = Arc::new(AtomicBool::new(false));
        let monitor_stop = Arc::clone(&stop);
        let monitor_violation = Arc::clone(&violation);
        let handle = thread::spawn(move || {
            let watcher = ProtectedCreateWatcher::new(&targets);
            while !monitor_stop.load(Ordering::SeqCst) {
                for target in &targets {
                    if remove_protected_create_target_best_effort(target).is_some() {
                        monitor_violation.store(true, Ordering::SeqCst);
                    }
                }
                if let Some(watcher) = &watcher {
                    watcher.wait_for_create_event(&monitor_stop);
                } else {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        });

        Some(Self {
            stop,
            violation,
            handle,
        })
    }

    pub(crate) fn stop(self) -> bool {
        self.stop.store(true, Ordering::SeqCst);
        self.handle
            .join()
            .unwrap_or_else(|_| panic!("protected create monitor thread panicked"));
        self.violation.load(Ordering::SeqCst)
    }
}

fn register_synthetic_mount_targets(
    targets: &[SyntheticMountTarget],
) -> Vec<SyntheticMountTargetRegistration> {
    with_metadata_guard_registry_lock(|| {
        targets
            .iter()
            .map(|target| {
                let marker_dir = metadata_guard_marker_dir(target.path());
                fs::create_dir_all(&marker_dir).unwrap_or_else(|err| {
                    panic!(
                        "failed to create metadata guard marker directory {}: {err}",
                        marker_dir.display()
                    )
                });
                let target = if target.preserves_pre_existing_path()
                    && metadata_guard_marker_dir_has_active_synthetic_owner(&marker_dir)
                {
                    match target.kind() {
                        SyntheticMountTargetKind::EmptyFile => {
                            SyntheticMountTarget::missing(target.path())
                        }
                        SyntheticMountTargetKind::EmptyDirectory => {
                            SyntheticMountTarget::missing_empty_directory(target.path())
                        }
                    }
                } else {
                    target.clone()
                };
                let marker_file = marker_dir.join(std::process::id().to_string());
                fs::write(&marker_file, synthetic_mount_marker_contents(&target)).unwrap_or_else(
                    |err| {
                        panic!(
                            "failed to register synthetic bubblewrap mount target {}: {err}",
                            target.path().display()
                        )
                    },
                );
                SyntheticMountTargetRegistration {
                    target,
                    marker_file,
                    marker_dir,
                }
            })
            .collect()
    })
}

fn register_protected_create_targets(
    targets: &[ProtectedCreateTarget],
) -> Vec<ProtectedCreateTargetRegistration> {
    with_metadata_guard_registry_lock(|| {
        targets
            .iter()
            .map(|target| {
                let marker_dir = metadata_guard_marker_dir(target.path());
                fs::create_dir_all(&marker_dir).unwrap_or_else(|err| {
                    panic!(
                        "failed to create protected create marker directory {}: {err}",
                        marker_dir.display()
                    )
                });
                let marker_file = marker_dir.join(std::process::id().to_string());
                fs::write(&marker_file, PROTECTED_CREATE_MARKER).unwrap_or_else(|err| {
                    panic!(
                        "failed to register protected create target {}: {err}",
                        target.path().display()
                    )
                });
                ProtectedCreateTargetRegistration {
                    target: target.clone(),
                    marker_file,
                    marker_dir,
                }
            })
            .collect()
    })
}

fn synthetic_mount_marker_contents(target: &SyntheticMountTarget) -> &'static [u8] {
    if target.preserves_pre_existing_path() {
        METADATA_GUARD_MARKER_EXISTING
    } else {
        METADATA_GUARD_MARKER_SYNTHETIC
    }
}

fn metadata_guard_marker_dir_has_active_synthetic_owner(marker_dir: &Path) -> bool {
    metadata_guard_marker_dir_has_active_process_matching(marker_dir, |path| match fs::read(path) {
        Ok(contents) => contents == METADATA_GUARD_MARKER_SYNTHETIC,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => panic!(
            "failed to read metadata guard marker {}: {err}",
            path.display()
        ),
    })
}

fn metadata_guard_marker_dir_has_active_process(marker_dir: &Path) -> bool {
    metadata_guard_marker_dir_has_active_process_matching(marker_dir, |_| true)
}

fn metadata_guard_marker_dir_has_active_process_matching(
    marker_dir: &Path,
    matches_marker: impl Fn(&Path) -> bool,
) -> bool {
    let entries = match fs::read_dir(marker_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return false,
        Err(err) => panic!(
            "failed to read metadata guard marker directory {}: {err}",
            marker_dir.display()
        ),
    };
    for entry in entries {
        let entry = entry.unwrap_or_else(|err| {
            panic!(
                "failed to read metadata guard marker in {}: {err}",
                marker_dir.display()
            )
        });
        let path = entry.path();
        let Some(pid) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        if !process_is_active(pid) {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => panic!(
                    "failed to remove stale metadata guard marker {}: {err}",
                    path.display()
                ),
            }
            continue;
        }
        let matches_marker = matches_marker(&path);
        if matches_marker {
            return true;
        }
    }
    false
}

fn cleanup_synthetic_mount_targets(targets: &[SyntheticMountTargetRegistration]) {
    with_metadata_guard_registry_lock(|| {
        for target in targets.iter().rev() {
            match fs::remove_file(&target.marker_file) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => panic!(
                    "failed to unregister synthetic bubblewrap mount target {}: {err}",
                    target.target.path().display()
                ),
            }
        }

        for target in targets.iter().rev() {
            if metadata_guard_marker_dir_has_active_process(&target.marker_dir) {
                continue;
            }
            remove_synthetic_mount_target(&target.target);
            match fs::remove_dir(&target.marker_dir) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                Err(err) => panic!(
                    "failed to remove metadata guard marker directory {}: {err}",
                    target.marker_dir.display()
                ),
            }
        }
    });
}

fn cleanup_protected_create_targets(targets: &[ProtectedCreateTargetRegistration]) -> bool {
    with_metadata_guard_registry_lock(|| {
        for target in targets.iter().rev() {
            match fs::remove_file(&target.marker_file) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => panic!(
                    "failed to unregister protected create target {}: {err}",
                    target.target.path().display()
                ),
            }
        }

        let mut violation = false;
        for target in targets.iter().rev() {
            if metadata_guard_marker_dir_has_active_process(&target.marker_dir) {
                if target.target.path().exists() {
                    violation = true;
                }
                continue;
            }
            violation |= remove_protected_create_target(&target.target);
            match fs::remove_dir(&target.marker_dir) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                Err(err) => panic!(
                    "failed to remove protected create marker directory {}: {err}",
                    target.marker_dir.display()
                ),
            }
        }
        violation
    })
}

fn remove_protected_create_target(target: &ProtectedCreateTarget) -> bool {
    for attempt in 0..100 {
        match try_remove_protected_create_target(target) {
            Ok(removal) => return removal.is_some(),
            Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty && attempt < 99 => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(err) => {
                panic!(
                    "failed to remove protected create target {}: {err}",
                    target.path().display()
                );
            }
        }
    }
    unreachable!("protected create removal retry loop should return or panic")
}

fn remove_protected_create_target_best_effort(
    target: &ProtectedCreateTarget,
) -> Option<ProtectedCreateRemoval> {
    for _ in 0..100 {
        match try_remove_protected_create_target(target) {
            Ok(removal) => return removal,
            Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return Some(ProtectedCreateRemoval::Other),
        }
    }
    Some(ProtectedCreateRemoval::Other)
}

fn try_remove_protected_create_target(
    target: &ProtectedCreateTarget,
) -> std::io::Result<Option<ProtectedCreateRemoval>> {
    let path = target.path();
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };

    let removal = if metadata.is_dir() {
        ProtectedCreateRemoval::Directory
    } else {
        ProtectedCreateRemoval::Other
    };
    let result = if removal == ProtectedCreateRemoval::Directory {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    }
    Ok(Some(removal))
}

fn remove_synthetic_mount_target(target: &SyntheticMountTarget) {
    let path = target.path();
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => panic!(
            "failed to inspect synthetic bubblewrap mount target {}: {err}",
            path.display()
        ),
    };
    if !target.should_remove_after_bwrap(&metadata) {
        return;
    }
    match target.kind() {
        SyntheticMountTargetKind::EmptyFile => match fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => panic!(
                "failed to remove synthetic bubblewrap mount target {}: {err}",
                path.display()
            ),
        },
        SyntheticMountTargetKind::EmptyDirectory => match fs::remove_dir(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
            Err(err) => panic!(
                "failed to remove synthetic bubblewrap mount target {}: {err}",
                path.display()
            ),
        },
    }
}

fn process_is_active(pid: libc::pid_t) -> bool {
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    let err = std::io::Error::last_os_error();
    !matches!(err.raw_os_error(), Some(libc::ESRCH))
}

fn with_metadata_guard_registry_lock<T>(f: impl FnOnce() -> T) -> T {
    let registry_root = metadata_guard_registry_root();
    fs::create_dir_all(&registry_root).unwrap_or_else(|err| {
        panic!(
            "failed to create metadata guard registry {}: {err}",
            registry_root.display()
        )
    });
    let lock_path = registry_root.join("lock");
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap_or_else(|err| {
            panic!(
                "failed to open metadata guard registry lock {}: {err}",
                lock_path.display()
            )
        });
    if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX) } < 0 {
        let err = std::io::Error::last_os_error();
        panic!(
            "failed to lock metadata guard registry {}: {err}",
            lock_path.display()
        );
    }
    let result = f();
    if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN) } < 0 {
        let err = std::io::Error::last_os_error();
        panic!(
            "failed to unlock metadata guard registry {}: {err}",
            lock_path.display()
        );
    }
    result
}

fn metadata_guard_marker_dir(path: &Path) -> PathBuf {
    metadata_guard_registry_root().join(format!("{:016x}", hash_path(path)))
}

fn metadata_guard_registry_root() -> PathBuf {
    std::env::temp_dir().join("codex-bwrap-synthetic-mount-targets")
}

fn hash_path(path: &Path) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.as_os_str().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
#[path = "metadata_guard_tests.rs"]
mod tests;
