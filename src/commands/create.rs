use std::path::PathBuf;

use chrono::Utc;

use crate::error::BotError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::rootfs;

/// All available environment IDs
pub const ALL_ENVIRONMENTS: &[&str] = &["rust", "nodejs", "python-uv", "go"];

/// Create a sandbox with all environments (backward-compatible CLI)
pub fn run(name: &str, workspace: Option<PathBuf>) -> Result<(), BotError> {
    let envs: Vec<String> = ALL_ENVIRONMENTS.iter().map(|s| s.to_string()).collect();
    run_with_envs(name, workspace, &envs)
}

/// Create a sandbox with selected environments
pub fn run_with_envs(
    name: &str,
    workspace: Option<PathBuf>,
    environments: &[String],
) -> Result<(), BotError> {
    validate_name(name)?;
    preflight_checks()?;

    let sandbox_dir = paths::sandbox_dir(name);
    if sandbox_dir.exists() {
        return Err(BotError::SandboxExists(name.to_string()));
    }

    let tarball = rootfs::ensure_rootfs_cached()?;

    let rootfs_path = paths::rootfs_dir(name);
    std::fs::create_dir_all(&rootfs_path)?;
    rootfs::extract_rootfs(&tarball, &rootfs_path)?;

    rootfs::post_setup_rootfs(&rootfs_path, name, environments)?;

    let config = SandboxConfig {
        name: name.to_string(),
        created_at: Utc::now(),
        rootfs_url: rootfs::rootfs_url().to_string(),
        workspace: workspace
            .or_else(|| std::env::current_dir().ok())
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p)),
        environments: environments.to_vec(),
    };
    config.save()?;

    eprintln!("Sandbox '{}' created successfully", name);

    let script = rootfs::build_provision_script(environments);
    eprintln!("Provisioning: {}...", environments.join(", "));
    let exit_code = crate::commands::enter::run_command(name, &script)?;
    if exit_code != 0 {
        return Err(BotError::ProvisionFailed(exit_code));
    }

    eprintln!("Sandbox '{}' is ready!", name);
    Ok(())
}

pub fn preflight_checks() -> Result<(), BotError> {
    for bin in &["newuidmap", "newgidmap"] {
        if std::process::Command::new("which")
            .arg(bin)
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            return Err(BotError::NamespaceError(format!(
                "{bin} not found. Install with: sudo apt install uidmap"
            )));
        }
    }
    Ok(())
}

pub fn validate_name(name: &str) -> Result<(), BotError> {
    if name.is_empty() {
        return Err(BotError::InvalidName(
            name.to_string(),
            "name cannot be empty".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(BotError::InvalidName(
            name.to_string(),
            "name must contain only alphanumeric characters, hyphens, and underscores".to_string(),
        ));
    }
    if name.starts_with('-') || name.starts_with('.') {
        return Err(BotError::InvalidName(
            name.to_string(),
            "name must not start with - or .".to_string(),
        ));
    }
    if name == ".." || name.contains("..") {
        return Err(BotError::InvalidName(
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
