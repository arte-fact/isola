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

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Provisioning failed with exit code {0}")]
    ProvisionFailed(i32),
}

#[cfg(target_os = "linux")]
impl From<nix::Error> for IsolaError {
    fn from(e: nix::Error) -> Self {
        IsolaError::NamespaceError(e.to_string())
    }
}
