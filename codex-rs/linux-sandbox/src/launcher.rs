use std::ffi::CStr;
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use codex_sandboxing::find_system_bwrap_in_path;
use codex_utils_absolute_path::AbsolutePathBuf;
use sha2::Digest as _;
use sha2::Sha256;

const SHA256_HEX_LEN: usize = 64;
const NULL_SHA256_DIGEST: [u8; 32] = [0; 32];

#[derive(Debug, Clone, PartialEq, Eq)]
enum BubblewrapLauncher {
    System(SystemBwrapLauncher),
    Bundled(BundledBwrapLauncher),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SystemBwrapLauncher {
    program: AbsolutePathBuf,
    supports_argv0: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BundledBwrapLauncher {
    program: AbsolutePathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SystemBwrapCapabilities {
    supports_argv0: bool,
    supports_perms: bool,
}

pub(crate) fn exec_bwrap(argv: Vec<String>, preserved_files: Vec<File>) -> ! {
    match preferred_bwrap_launcher() {
        BubblewrapLauncher::System(launcher) => {
            exec_system_bwrap(&launcher.program, argv, preserved_files)
        }
        BubblewrapLauncher::Bundled(launcher) => {
            exec_bundled_bwrap(&launcher.program, argv, preserved_files)
        }
        BubblewrapLauncher::Unavailable => {
            panic!(
                "bubblewrap is unavailable: no system bwrap was found on PATH and no bundled \
                 codex-resources/bwrap binary was found next to the Codex executable"
            )
        }
    }
}

fn preferred_bwrap_launcher() -> BubblewrapLauncher {
    static LAUNCHER: OnceLock<BubblewrapLauncher> = OnceLock::new();
    LAUNCHER
        .get_or_init(|| {
            if let Some(path) = find_system_bwrap_in_path()
                && let Some(launcher) = system_bwrap_launcher_for_path(&path)
            {
                return BubblewrapLauncher::System(launcher);
            }

            match bundled_bwrap_launcher() {
                Some(launcher) => BubblewrapLauncher::Bundled(launcher),
                None => BubblewrapLauncher::Unavailable,
            }
        })
        .clone()
}

fn system_bwrap_launcher_for_path(system_bwrap_path: &Path) -> Option<SystemBwrapLauncher> {
    system_bwrap_launcher_for_path_with_probe(system_bwrap_path, system_bwrap_capabilities)
}

fn system_bwrap_launcher_for_path_with_probe(
    system_bwrap_path: &Path,
    system_bwrap_capabilities: impl FnOnce(&Path) -> Option<SystemBwrapCapabilities>,
) -> Option<SystemBwrapLauncher> {
    if !system_bwrap_path.is_file() {
        return None;
    }

    let Some(SystemBwrapCapabilities {
        supports_argv0,
        supports_perms: true,
    }) = system_bwrap_capabilities(system_bwrap_path)
    else {
        return None;
    };
    let system_bwrap_path = match AbsolutePathBuf::from_absolute_path(system_bwrap_path) {
        Ok(path) => path,
        Err(err) => panic!(
            "failed to normalize system bubblewrap path {}: {err}",
            system_bwrap_path.display()
        ),
    };
    Some(SystemBwrapLauncher {
        program: system_bwrap_path,
        supports_argv0,
    })
}

pub(crate) fn preferred_bwrap_supports_argv0() -> bool {
    match preferred_bwrap_launcher() {
        BubblewrapLauncher::System(launcher) => launcher.supports_argv0,
        BubblewrapLauncher::Bundled(_) | BubblewrapLauncher::Unavailable => true,
    }
}

fn bundled_bwrap_launcher() -> Option<BundledBwrapLauncher> {
    let current_exe = std::env::current_exe().ok()?;
    find_bundled_bwrap_for_exe(&current_exe).map(|program| BundledBwrapLauncher { program })
}

fn find_bundled_bwrap_for_exe(exe: &Path) -> Option<AbsolutePathBuf> {
    bundled_bwrap_candidates_for_exe(exe)
        .into_iter()
        .find(|candidate| is_executable_file(candidate))
        .map(|path| {
            AbsolutePathBuf::from_absolute_path(&path).unwrap_or_else(|err| {
                panic!(
                    "failed to normalize bundled bubblewrap path {}: {err}",
                    path.display()
                )
            })
        })
}

fn bundled_bwrap_candidates_for_exe(exe: &Path) -> Vec<PathBuf> {
    let Some(exe_dir) = exe.parent() else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    candidates.push(exe_dir.join("codex-resources").join("bwrap"));
    if let Some(package_target_dir) = exe_dir.parent() {
        candidates.push(package_target_dir.join("codex-resources").join("bwrap"));
    }
    candidates.push(exe_dir.join("bwrap"));
    candidates
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

fn system_bwrap_capabilities(system_bwrap_path: &Path) -> Option<SystemBwrapCapabilities> {
    // bubblewrap added `--argv0` in v0.9.0:
    // https://github.com/containers/bubblewrap/releases/tag/v0.9.0
    // Older distro packages (for example Ubuntu 20.04/22.04) ship builds that
    // reject `--argv0`, so use the system binary's no-argv0 compatibility path
    // in that case.
    let output = match Command::new(system_bwrap_path).arg("--help").output() {
        Ok(output) => output,
        Err(_) => return None,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Some(SystemBwrapCapabilities {
        supports_argv0: stdout.contains("--argv0") || stderr.contains("--argv0"),
        supports_perms: stdout.contains("--perms") || stderr.contains("--perms"),
    })
}

fn exec_system_bwrap(
    program: &AbsolutePathBuf,
    argv: Vec<String>,
    preserved_files: Vec<File>,
) -> ! {
    // System bwrap runs across an exec boundary, so preserved fds must survive exec.
    make_files_inheritable(&preserved_files);

    let program_path = program.as_path().display().to_string();
    let program = CString::new(program.as_path().as_os_str().as_bytes())
        .unwrap_or_else(|err| panic!("invalid system bubblewrap path: {err}"));
    let cstrings = argv_to_cstrings(&argv);
    let mut argv_ptrs: Vec<*const c_char> = cstrings
        .iter()
        .map(CString::as_c_str)
        .map(CStr::as_ptr)
        .collect();
    argv_ptrs.push(std::ptr::null());

    // SAFETY: `program` and every entry in `argv_ptrs` are valid C strings for
    // the duration of the call. On success `execv` does not return.
    unsafe {
        libc::execv(program.as_ptr(), argv_ptrs.as_ptr());
    }
    let err = std::io::Error::last_os_error();
    panic!("failed to exec system bubblewrap {program_path}: {err}");
}

fn exec_bundled_bwrap(
    program: &AbsolutePathBuf,
    argv: Vec<String>,
    preserved_files: Vec<File>,
) -> ! {
    let bwrap_file = File::open(program.as_path()).unwrap_or_else(|err| {
        panic!(
            "failed to open bundled bubblewrap {}: {err}",
            program.as_path().display()
        )
    });
    verify_bundled_bwrap_digest(
        &bwrap_file,
        expected_bundled_bwrap_sha256(),
        program.as_path(),
    )
    .unwrap_or_else(|err| panic!("{err}"));

    make_files_inheritable(&preserved_files);

    let fd_path = format!("/proc/self/fd/{}", bwrap_file.as_raw_fd());
    let program_cstring = CString::new(fd_path.as_str())
        .unwrap_or_else(|err| panic!("invalid bundled bubblewrap fd path: {err}"));
    let cstrings = argv_to_cstrings(&argv);
    let mut argv_ptrs: Vec<*const c_char> = cstrings
        .iter()
        .map(CString::as_c_str)
        .map(CStr::as_ptr)
        .collect();
    argv_ptrs.push(std::ptr::null());

    // SAFETY: `program_cstring` and every entry in `argv_ptrs` are valid C
    // strings for the duration of the call. On success `execv` does not return.
    unsafe {
        libc::execv(program_cstring.as_ptr(), argv_ptrs.as_ptr());
    }
    let err = std::io::Error::last_os_error();
    panic!(
        "failed to exec bundled bubblewrap {} via {fd_path}: {err}",
        program.as_path().display()
    );
}

fn expected_bundled_bwrap_sha256() -> Option<[u8; 32]> {
    static EXPECTED: OnceLock<Option<[u8; 32]>> = OnceLock::new();
    *EXPECTED.get_or_init(|| {
        let Some(raw_digest) = option_env!("CODEX_BWRAP_SHA256") else {
            return None;
        };
        let digest = parse_sha256_hex(raw_digest)
            .unwrap_or_else(|err| panic!("invalid CODEX_BWRAP_SHA256 value: {err}"));
        (digest != NULL_SHA256_DIGEST).then_some(digest)
    })
}

fn verify_bundled_bwrap_digest(
    file: &File,
    expected: Option<[u8; 32]>,
    path: &Path,
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };

    let mut file = file
        .try_clone()
        .map_err(|err| format!("failed to clone bundled bubblewrap fd: {err}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer).map_err(|err| {
            format!(
                "failed to read bundled bubblewrap {} for digest verification: {err}",
                path.display()
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let actual: [u8; 32] = hasher.finalize().into();
    if actual == expected {
        return Ok(());
    }

    Err(format!(
        "bundled bubblewrap digest mismatch for {}: expected sha256:{}, got sha256:{}",
        path.display(),
        bytes_to_hex(&expected),
        bytes_to_hex(&actual),
    ))
}

fn parse_sha256_hex(raw: &str) -> Result<[u8; 32], String> {
    if raw.len() != SHA256_HEX_LEN {
        return Err(format!(
            "expected {SHA256_HEX_LEN} hex characters, got {}",
            raw.len()
        ));
    }

    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&raw[start..start + 2], 16)
            .map_err(|err| format!("invalid hex byte at offset {start}: {err}"))?;
    }
    Ok(digest)
}

fn bytes_to_hex(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(SHA256_HEX_LEN);
    for byte in bytes {
        hex.push(HEX[(byte >> 4) as usize] as char);
        hex.push(HEX[(byte & 0x0f) as usize] as char);
    }
    hex
}

fn argv_to_cstrings(argv: &[String]) -> Vec<CString> {
    let mut cstrings: Vec<CString> = Vec::with_capacity(argv.len());
    for arg in argv {
        match CString::new(arg.as_str()) {
            Ok(value) => cstrings.push(value),
            Err(err) => panic!("failed to convert argv to CString: {err}"),
        }
    }
    cstrings
}

fn make_files_inheritable(files: &[File]) {
    for file in files {
        clear_cloexec(file.as_raw_fd());
    }
}

fn clear_cloexec(fd: libc::c_int) {
    // SAFETY: `fd` is an owned descriptor kept alive by `files`.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        let err = std::io::Error::last_os_error();
        panic!("failed to read fd flags for preserved bubblewrap file descriptor {fd}: {err}");
    }
    let cleared_flags = flags & !libc::FD_CLOEXEC;
    if cleared_flags == flags {
        return;
    }

    // SAFETY: `fd` is valid and we are only clearing FD_CLOEXEC.
    let result = unsafe { libc::fcntl(fd, libc::F_SETFD, cleared_flags) };
    if result < 0 {
        let err = std::io::Error::last_os_error();
        panic!("failed to clear CLOEXEC for preserved bubblewrap file descriptor {fd}: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::NamedTempFile;
    use tempfile::tempdir;

    #[test]
    fn prefers_system_bwrap_when_help_lists_argv0() {
        let fake_bwrap = NamedTempFile::new().expect("temp file");
        let fake_bwrap_path = fake_bwrap.path();
        let expected = AbsolutePathBuf::from_absolute_path(fake_bwrap_path).expect("absolute");

        assert_eq!(
            system_bwrap_launcher_for_path_with_probe(fake_bwrap_path, |_| {
                Some(SystemBwrapCapabilities {
                    supports_argv0: true,
                    supports_perms: true,
                })
            }),
            Some(SystemBwrapLauncher {
                program: expected,
                supports_argv0: true,
            })
        );
    }

    #[test]
    fn prefers_system_bwrap_when_system_bwrap_lacks_argv0() {
        let fake_bwrap = NamedTempFile::new().expect("temp file");
        let fake_bwrap_path = fake_bwrap.path();

        assert_eq!(
            system_bwrap_launcher_for_path_with_probe(fake_bwrap_path, |_| {
                Some(SystemBwrapCapabilities {
                    supports_argv0: false,
                    supports_perms: true,
                })
            }),
            Some(SystemBwrapLauncher {
                program: AbsolutePathBuf::from_absolute_path(fake_bwrap_path).expect("absolute"),
                supports_argv0: false,
            })
        );
    }

    #[test]
    fn ignores_system_bwrap_when_system_bwrap_lacks_perms() {
        let fake_bwrap = NamedTempFile::new().expect("temp file");

        assert_eq!(
            system_bwrap_launcher_for_path_with_probe(fake_bwrap.path(), |_| {
                Some(SystemBwrapCapabilities {
                    supports_argv0: false,
                    supports_perms: false,
                })
            }),
            None
        );
    }

    #[test]
    fn ignores_system_bwrap_when_system_bwrap_is_missing() {
        assert_eq!(
            system_bwrap_launcher_for_path(Path::new("/definitely/not/a/bwrap")),
            None
        );
    }

    #[test]
    fn finds_standalone_bundled_bwrap_next_to_exe_resources() {
        let temp_dir = tempdir().expect("temp dir");
        let exe = temp_dir.path().join("codex");
        let expected_bwrap = temp_dir.path().join("codex-resources").join("bwrap");
        write_executable(&exe);
        write_executable(&expected_bwrap);

        assert_eq!(
            find_bundled_bwrap_for_exe(&exe),
            Some(AbsolutePathBuf::from_absolute_path(&expected_bwrap).expect("absolute"))
        );
    }

    #[test]
    fn finds_npm_bundled_bwrap_next_to_target_vendor_dir() {
        let temp_dir = tempdir().expect("temp dir");
        let target_dir = temp_dir.path().join("vendor/x86_64-unknown-linux-musl");
        let exe = target_dir.join("codex").join("codex");
        let expected_bwrap = target_dir.join("codex-resources").join("bwrap");
        write_executable(&exe);
        write_executable(&expected_bwrap);

        assert_eq!(
            find_bundled_bwrap_for_exe(&exe),
            Some(AbsolutePathBuf::from_absolute_path(&expected_bwrap).expect("absolute"))
        );
    }

    #[test]
    fn finds_adjacent_dev_bwrap() {
        let temp_dir = tempdir().expect("temp dir");
        let exe = temp_dir.path().join("codex");
        let expected_bwrap = temp_dir.path().join("bwrap");
        write_executable(&exe);
        write_executable(&expected_bwrap);

        assert_eq!(
            find_bundled_bwrap_for_exe(&exe),
            Some(AbsolutePathBuf::from_absolute_path(&expected_bwrap).expect("absolute"))
        );
    }

    #[test]
    fn bundled_digest_verification_skips_missing_expected_digest() {
        let file = NamedTempFile::new().expect("temp file");
        fs::write(file.path(), b"contents").expect("write file");

        verify_bundled_bwrap_digest(file.as_file(), /*expected*/ None, file.path())
            .expect("missing digest should skip verification");
    }

    #[test]
    fn bundled_digest_verification_accepts_matching_digest() {
        let file = NamedTempFile::new().expect("temp file");
        fs::write(file.path(), b"contents").expect("write file");
        let expected: [u8; 32] = Sha256::digest(b"contents").into();

        verify_bundled_bwrap_digest(file.as_file(), Some(expected), file.path())
            .expect("matching digest should verify");
    }

    #[test]
    fn bundled_digest_verification_rejects_mismatched_digest() {
        let file = NamedTempFile::new().expect("temp file");
        fs::write(file.path(), b"contents").expect("write file");

        let err = verify_bundled_bwrap_digest(file.as_file(), Some([0xab; 32]), file.path())
            .expect_err("mismatched digest should fail");
        assert!(err.contains("bundled bubblewrap digest mismatch"));
    }

    #[test]
    fn parses_sha256_hex_digest() {
        assert_eq!(parse_sha256_hex(&"ab".repeat(32)), Ok([0xab; 32]));
        assert_eq!(parse_sha256_hex(&"00".repeat(32)), Ok(NULL_SHA256_DIGEST));
        assert!(parse_sha256_hex("ab").is_err());
        assert!(parse_sha256_hex(&format!("{}xx", "00".repeat(31))).is_err());
    }

    #[test]
    fn preserved_files_are_made_inheritable_for_system_exec() {
        let file = NamedTempFile::new().expect("temp file");
        set_cloexec(file.as_file().as_raw_fd());

        make_files_inheritable(std::slice::from_ref(file.as_file()));

        assert_eq!(fd_flags(file.as_file().as_raw_fd()) & libc::FD_CLOEXEC, 0);
    }

    fn write_executable(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, b"").expect("write executable");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("set executable permissions");
    }

    fn set_cloexec(fd: libc::c_int) {
        let flags = fd_flags(fd);
        // SAFETY: `fd` is valid for the duration of the test.
        let result = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        if result < 0 {
            let err = std::io::Error::last_os_error();
            panic!("failed to set CLOEXEC for test fd {fd}: {err}");
        }
    }

    fn fd_flags(fd: libc::c_int) -> libc::c_int {
        // SAFETY: `fd` is valid for the duration of the test.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            let err = std::io::Error::last_os_error();
            panic!("failed to read fd flags for test fd {fd}: {err}");
        }
        flags
    }
}
