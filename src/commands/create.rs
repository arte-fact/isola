use std::path::PathBuf;

use chrono::Utc;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::backend;
use crate::sandbox::config::SandboxConfig;

/// All available environment IDs
pub const ALL_ENVIRONMENTS: &[&str] = &["rust", "nodejs", "python-uv", "go"];

/// Create a sandbox with all environments (backward-compatible CLI)
pub fn run(name: &str, workspace: Option<PathBuf>) -> Result<(), IsolaError> {
    let envs: Vec<String> = ALL_ENVIRONMENTS.iter().map(|s| s.to_string()).collect();
    run_with_envs(name, workspace, &envs)
}

/// Create a sandbox with selected environments
pub fn run_with_envs(
    name: &str,
    workspace: Option<PathBuf>,
    environments: &[String],
) -> Result<(), IsolaError> {
    validate_name(name)?;

    let backend = backend::create_backend();
    backend.preflight_checks()?;

    let sandbox_dir = paths::sandbox_dir(name);
    if sandbox_dir.exists() {
        return Err(IsolaError::SandboxExists(name.to_string()));
    }

    let workspace = workspace
        .or_else(|| std::env::current_dir().ok())
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p));

    backend.create_environment(name, workspace.as_deref())?;
    backend.write_sandbox_files(name, environments)?;

    let config = SandboxConfig {
        name: name.to_string(),
        created_at: Utc::now(),
        rootfs_url: backend.rootfs_url().to_string(),
        workspace,
        environments: environments.to_vec(),
        backend: backend.backend_name().to_string(),
    };
    config.save()?;

    eprintln!("Sandbox '{}' created successfully", name);

    let script = backend.build_provision_script(environments);
    eprintln!("Provisioning: {}...", environments.join(", "));
    let exit_code = backend.run_command(name, &script)?;
    if exit_code != 0 {
        return Err(IsolaError::ProvisionFailed(exit_code));
    }

    eprintln!("Sandbox '{}' is ready!", name);
    Ok(())
}

pub fn validate_name(name: &str) -> Result<(), IsolaError> {
    if name.is_empty() {
        return Err(IsolaError::InvalidName(
            name.to_string(),
            "name cannot be empty".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(IsolaError::InvalidName(
            name.to_string(),
            "name must contain only alphanumeric characters, hyphens, and underscores".to_string(),
        ));
    }
    if name.starts_with('-') || name.starts_with('.') {
        return Err(IsolaError::InvalidName(
            name.to_string(),
            "name must not start with - or .".to_string(),
        ));
    }
    if name == ".." || name.contains("..") {
        return Err(IsolaError::InvalidName(
            name.to_string(),
            "name must not contain '..'".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        assert!(validate_name("my-sandbox").is_ok());
        assert!(validate_name("test_123").is_ok());
        assert!(validate_name("a").is_ok());
        assert!(validate_name("ABC").is_ok());
    }

    #[test]
    fn empty_name_rejected() {
        assert!(validate_name("").is_err());
    }

    #[test]
    fn special_chars_rejected() {
        assert!(validate_name("my sandbox").is_err());
        assert!(validate_name("foo/bar").is_err());
        assert!(validate_name("foo@bar").is_err());
        assert!(validate_name("hello!").is_err());
    }

    #[test]
    fn leading_dash_rejected() {
        assert!(validate_name("-bad").is_err());
    }

    #[test]
    fn leading_dot_rejected() {
        assert!(validate_name(".hidden").is_err());
    }

    #[test]
    fn dotdot_rejected() {
        assert!(validate_name("..").is_err());
        assert!(validate_name("a..b").is_err());
    }
}
