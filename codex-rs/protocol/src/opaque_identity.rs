use std::ffi::OsString;

/// Binary gRPC metadata key used to forward the caller's opaque identity to
/// remote core contract implementations.
pub const CODEX_CORE_IDENTITY_HEADER: &str = "x-codex-core-identity-bin";

/// Opaque identity supplied by a Codex core API caller.
///
/// Codex core treats this as uninterpreted bytes and only forwards it to remote
/// contract implementations that need to perform their own authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpaqueIdentity {
    bytes: Vec<u8>,
}

impl OpaqueIdentity {
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    pub fn from_os_string(value: OsString) -> Self {
        Self::from_bytes(os_string_to_bytes(value))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(unix)]
fn os_string_to_bytes(value: OsString) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    value.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_string_to_bytes(value: OsString) -> Vec<u8> {
    value.to_string_lossy().into_owned().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::OpaqueIdentity;
    use pretty_assertions::assert_eq;

    #[test]
    fn opaque_identity_preserves_bytes() {
        let identity = OpaqueIdentity::from_bytes(b"tenant-key-\x00\xff".to_vec());

        assert_eq!(identity.as_bytes(), &b"tenant-key-\x00\xff"[..]);
    }

    #[cfg(unix)]
    #[test]
    fn opaque_identity_preserves_unix_argv_bytes() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let identity =
            OpaqueIdentity::from_os_string(OsString::from_vec(b"tenant-key-\xff".to_vec()));

        assert_eq!(identity.as_bytes(), &b"tenant-key-\xff"[..]);
    }
}
