use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum IsolaError {
    #[error("Sandbox '{0}' already exists")]
    SandboxExists(String),

    #[error("Sandbox '{0}' not found")]
    SandboxNotFound(String),

    #[error("Invalid sandbox name '{0}': {1}")]
    InvalidName(String, String),

    #[error("Rootfs download failed: {0}")]
    DownloadFailed(#[from] reqwest::Error),

    #[error("Rootfs extraction failed: {0}")]
    #[cfg(target_os = "linux")]
    ExtractionFailed(String),

    #[error("Namespace setup failed: {0}")]
    #[cfg(target_os = "linux")]
    NamespaceError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// IO error with operation + path context.
    #[error("failed to {op} '{}': {source}", path.display())]
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    IoAt {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Provisioning failed with exit code {0}")]
    ProvisionFailed(i32),

    #[error("Plugin error: {0}")]
    PluginError(String),
}

/// Attach operation + path context to a `std::io::Result`.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub trait IoContext<T> {
    fn io_ctx(self, op: &'static str, path: impl AsRef<Path>) -> Result<T, IsolaError>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn io_ctx(self, op: &'static str, path: impl AsRef<Path>) -> Result<T, IsolaError> {
        self.map_err(|source| IsolaError::IoAt {
            op,
            path: path.as_ref().to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_exists_display() {
        let e = IsolaError::SandboxExists("test".into());
        assert_eq!(e.to_string(), "Sandbox 'test' already exists");
    }

    #[test]
    fn sandbox_not_found_display() {
        let e = IsolaError::SandboxNotFound("dev".into());
        assert_eq!(e.to_string(), "Sandbox 'dev' not found");
    }

    #[test]
    fn invalid_name_display() {
        let e = IsolaError::InvalidName("a/b".into(), "contains slash".into());
        assert_eq!(e.to_string(), "Invalid sandbox name 'a/b': contains slash");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn extraction_failed_display() {
        let e = IsolaError::ExtractionFailed("corrupt tarball".into());
        assert_eq!(e.to_string(), "Rootfs extraction failed: corrupt tarball");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn namespace_error_display() {
        let e = IsolaError::NamespaceError("clone() failed".into());
        assert_eq!(e.to_string(), "Namespace setup failed: clone() failed");
    }

    #[test]
    fn config_error_display() {
        let e = IsolaError::ConfigError("missing field".into());
        assert_eq!(e.to_string(), "Configuration error: missing field");
    }

    #[test]
    fn provision_failed_display() {
        let e = IsolaError::ProvisionFailed(127);
        assert_eq!(e.to_string(), "Provisioning failed with exit code 127");
    }

    #[test]
    fn io_error_from_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e: IsolaError = io_err.into();
        assert!(matches!(e, IsolaError::Io(_)));
        assert!(e.to_string().contains("file not found"));
    }

    #[test]
    fn io_ctx_adds_path_and_op() {
        let result: std::io::Result<()> = Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No such file or directory (os error 2)",
        ));
        let e = result.io_ctx("read", "/etc/resolv.conf").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("read"), "missing op: {msg}");
        assert!(msg.contains("/etc/resolv.conf"), "missing path: {msg}");
        assert!(matches!(e, IsolaError::IoAt { .. }));
    }

    #[test]
    fn io_ctx_exposes_source_for_chain() {
        let result: std::io::Result<()> = Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        let e = result.io_ctx("open", "/some/path").unwrap_err();
        let src = std::error::Error::source(&e).expect("IoAt must expose source");
        assert!(src.to_string().contains("denied"));
    }
}

#[cfg(target_os = "linux")]
impl From<nix::Error> for IsolaError {
    fn from(e: nix::Error) -> Self {
        IsolaError::NamespaceError(e.to_string())
    }
}
