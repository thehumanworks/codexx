#![cfg(target_os = "windows")]

use crate::log_note;
use crate::path_normalization::normalize_spawn_cwd;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::os::windows::fs::MetadataExt as _;
use std::os::windows::process::CommandExt as _;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::path::Prefix;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

fn junction_name_for_path(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn junction_root_for_userprofile(userprofile: &str) -> PathBuf {
    PathBuf::from(userprofile)
        .join(".codex")
        .join(".sandbox")
        .join("cwd")
}

fn drive_letter(path: &Path) -> Option<char> {
    match path.components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                Some((drive as char).to_ascii_uppercase())
            }
            _ => None,
        },
        _ => None,
    }
}

fn system_drive_letter(system_drive: Option<&str>) -> Option<char> {
    drive_letter(Path::new(system_drive?))
}

fn should_materialize_junction(requested_cwd: &Path, system_drive: Option<&str>) -> bool {
    let Some(requested_drive) = drive_letter(requested_cwd) else {
        return false;
    };
    let Some(system_drive) = system_drive_letter(system_drive) else {
        return false;
    };
    requested_drive != system_drive
}

fn create_cwd_junction(requested_cwd: &Path, log_dir: Option<&Path>) -> Option<PathBuf> {
    let userprofile = std::env::var("USERPROFILE").ok()?;
    let junction_root = junction_root_for_userprofile(&userprofile);
    if let Err(err) = std::fs::create_dir_all(&junction_root) {
        log_note(
            &format!(
                "junction: failed to create {}: {err}",
                junction_root.display()
            ),
            log_dir,
        );
        return None;
    }

    let junction_path = junction_root.join(junction_name_for_path(requested_cwd));
    if junction_path.exists() {
        match std::fs::symlink_metadata(&junction_path) {
            Ok(md) if (md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 => {
                log_note(
                    &format!("junction: reusing existing {}", junction_path.display()),
                    log_dir,
                );
                return Some(junction_path);
            }
            Ok(_) => {
                log_note(
                    &format!(
                        "junction: existing path is not a reparse point, recreating {}",
                        junction_path.display()
                    ),
                    log_dir,
                );
            }
            Err(err) => {
                log_note(
                    &format!(
                        "junction: failed to stat existing {}: {err}",
                        junction_path.display()
                    ),
                    log_dir,
                );
                return None;
            }
        }

        if let Err(err) = std::fs::remove_dir(&junction_path) {
            log_note(
                &format!(
                    "junction: failed to remove existing {}: {err}",
                    junction_path.display()
                ),
                log_dir,
            );
            return None;
        }
    }

    let link = junction_path.to_string_lossy().to_string();
    let target = requested_cwd.to_string_lossy().to_string();
    let link_quoted = format!("\"{link}\"");
    let target_quoted = format!("\"{target}\"");
    log_note(
        &format!("junction: creating via cmd /c mklink /J {link_quoted} {target_quoted}"),
        log_dir,
    );
    let output = match std::process::Command::new("cmd")
        .raw_arg("/c")
        .raw_arg("mklink")
        .raw_arg("/J")
        .raw_arg(&link_quoted)
        .raw_arg(&target_quoted)
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            log_note(&format!("junction: mklink failed to run: {err}"), log_dir);
            return None;
        }
    };
    if output.status.success() && junction_path.exists() {
        log_note(
            &format!(
                "junction: created {} -> {}",
                junction_path.display(),
                requested_cwd.display()
            ),
            log_dir,
        );
        return Some(junction_path);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    log_note(
        &format!(
            "junction: mklink failed status={:?} stdout={} stderr={}",
            output.status,
            stdout.trim(),
            stderr.trim()
        ),
        log_dir,
    );
    None
}

pub(crate) fn effective_legacy_spawn_cwd(cwd: &Path, log_dir: Option<&Path>) -> PathBuf {
    let normalized_cwd = normalize_spawn_cwd(cwd);
    if should_materialize_junction(
        &normalized_cwd,
        std::env::var("SystemDrive").ok().as_deref(),
    ) {
        create_cwd_junction(&normalized_cwd, log_dir).unwrap_or_else(|| normalized_cwd.clone())
    } else {
        normalized_cwd
    }
}

#[cfg(test)]
mod tests {
    use super::effective_legacy_spawn_cwd;
    use super::should_materialize_junction;
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::path::PathBuf;

    #[test]
    fn skips_system_drive_workspaces() {
        assert!(!should_materialize_junction(
            Path::new(r"C:\repo"),
            Some("C:"),
        ));
    }

    #[test]
    fn uses_junction_for_non_system_drive_workspaces() {
        assert!(should_materialize_junction(
            Path::new(r"F:\repo"),
            Some("C:"),
        ));
    }

    #[test]
    fn skips_unc_paths() {
        assert!(!should_materialize_junction(
            Path::new(r"\\server\share\repo"),
            Some("C:"),
        ));
    }

    #[test]
    fn leaves_system_drive_paths_unchanged() {
        let cwd = PathBuf::from(r"C:\repo");
        assert_eq!(effective_legacy_spawn_cwd(&cwd, None), cwd);
    }
}
