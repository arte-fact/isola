use std::path::PathBuf;

use chrono::Utc;

use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::rootfs;

/// All available environment IDs
pub const ALL_ENVIRONMENTS: &[&str] = &["rust", "nodejs", "python-uv", "go"];

/// Create a sandbox with all environments (backward-compatible CLI)
pub fn run(name: &str, workspace: Option<PathBuf>) -> Result<(), IsolaError> {
    let envs: Vec<String> = ALL_ENVIRONMENTS.iter().map(|s| s.to_string()).collect();
    run_with_envs(name, workspace, &envs, false)
}

/// Create a sandbox with selected environments
pub fn run_with_envs(
    name: &str,
    workspace: Option<PathBuf>,
    environments: &[String],
    share_ssh: bool,
) -> Result<(), IsolaError> {
    use crate::progress::{self, CreationProgress};

    let progress = CreationProgress::new(name);

    validate_name(name)?;
    preflight_checks()?;
    progress.finish_step("Preflight checks passed");

    let sandbox_dir = paths::sandbox_dir(name);
    if sandbox_dir.exists() {
        return Err(IsolaError::SandboxExists(name.to_string()));
    }

    let tarball = rootfs::ensure_rootfs_cached_with_progress(&progress)?;

    progress.start_step("Extracting rootfs...");
    let rootfs_path = paths::rootfs_dir(name);
    std::fs::create_dir_all(&rootfs_path)?;
    rootfs::extract_rootfs(&tarball, &rootfs_path)?;
    progress.finish_step("Extracted rootfs");

    progress.start_step("Configuring rootfs...");
    rootfs::post_setup_rootfs(&rootfs_path, name, environments)?;
    progress.finish_step("Configured rootfs");

    let config = SandboxConfig {
        name: name.to_string(),
        created_at: Utc::now(),
        rootfs_url: rootfs::rootfs_url().to_string(),
        workspace: workspace
            .or_else(|| std::env::current_dir().ok())
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p)),
        environments: environments.to_vec(),
        share_ssh,
    };
    config.save()?;

    // Provision with captured output
    progress.start_provision();
    let script = rootfs::build_provision_script(environments);
    let child = crate::commands::enter::run_command_captured(name, &script)?;
    let (exit_code, last_lines) = progress::monitor_provisioning(child, &progress, environments)?;

    if exit_code != 0 {
        progress.finish_error(exit_code, &last_lines);
        return Err(IsolaError::ProvisionFailed(exit_code));
    }

    progress.finish_success(environments);
    Ok(())
}

pub fn preflight_checks() -> Result<(), IsolaError> {
    if !crate::sandbox::userns::has_uidmap_tools() {
        eprintln!(
            "Note: newuidmap/newgidmap not found (install with: sudo apt install uidmap).\n\
             The sandbox will use single-UID mapping (no root/user separation inside)."
        );
    }

    if crate::commands::setup_host::apparmor_userns_restricted()
        && !crate::commands::setup_host::has_apparmor_profile()
    {
        return Err(IsolaError::NamespaceError(
            "AppArmor restricts unprivileged user namespaces on this system.\n\
             Run `isola setup-host` to install the required AppArmor profile."
                .to_string(),
        ));
    }

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
