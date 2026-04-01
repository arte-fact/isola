use std::path::PathBuf;

use chrono::Utc;

use crate::error::IsolaError;
use crate::paths;
use crate::plugin::PluginRegistry;
use crate::sandbox::config::{SandboxConfig, SandboxShell};
use crate::sandbox::rootfs;

/// Create a sandbox with all non-neovim bundled environments (CLI shorthand)
pub fn run(name: &str, workspace: Option<PathBuf>, no_cache: bool) -> Result<(), IsolaError> {
    let registry = PluginRegistry::load()?;
    let envs: Vec<String> = registry
        .available_names()
        .into_iter()
        .filter(|n| *n != "neovim")
        .map(|s| s.to_string())
        .collect();
    run_with_envs(
        name,
        workspace,
        &envs,
        false,
        no_cache,
        &SandboxShell::default(),
        &registry,
    )
}

/// Create a sandbox with selected environments
pub fn run_with_envs(
    name: &str,
    workspace: Option<PathBuf>,
    environments: &[String],
    share_ssh: bool,
    no_cache: bool,
    shell: &SandboxShell,
    registry: &PluginRegistry,
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

    let rootfs_path = paths::rootfs_dir(name);
    std::fs::create_dir_all(&rootfs_path)?;

    // Try layered cache first, then legacy monolithic cache, then full provision
    let layer_status = if no_cache {
        None
    } else {
        Some(rootfs::check_layer_cache(environments, shell, registry))
    };

    let used_layered_cache = if let Some(ref status) = layer_status
        && status.all_cached()
    {
        // Fast path: all layers cached
        progress.start_step("Extracting cached layers...");
        for (_, layer_path) in &status.cached {
            rootfs::extract_rootfs(layer_path, &rootfs_path)?;
        }
        progress.finish_step("Extracted cached layers");
        true
    } else if !no_cache
        && let Some(cache_path) = rootfs::has_cached_provision(environments, shell.name())
    {
        // Legacy fallback: monolithic cache exists
        progress.start_step("Extracting cached rootfs...");
        rootfs::extract_rootfs(&cache_path, &rootfs_path)?;
        progress.finish_step("Extracted cached rootfs");
        false
    } else if let Some(ref status) = layer_status
        && !status.cached.is_empty()
    {
        // Partial layered cache: extract cached layers, build missing ones
        progress.start_step("Extracting cached layers...");
        for (_, layer_path) in &status.cached {
            rootfs::extract_rootfs(layer_path, &rootfs_path)?;
        }
        progress.finish_step("Extracted cached layers");

        // Build each uncached layer
        for layer_name in &status.uncached {
            let script = if layer_name == "base" {
                rootfs::build_base_layer_script(shell)
            } else {
                rootfs::build_env_layer_script(layer_name, registry).ok_or_else(|| {
                    IsolaError::PluginError(format!("no plugin found for '{layer_name}'"))
                })?
            };

            progress.start_step(&format!("Provisioning {layer_name}..."));

            // Save config early so run_command_captured can find the sandbox
            save_config(name, &workspace, environments, share_ssh, shell)?;

            let child = crate::commands::enter::run_command_captured(name, &script)?;
            let (exit_code, last_lines) =
                progress::monitor_provisioning(child, &progress, std::slice::from_ref(layer_name))?;
            if exit_code != 0 {
                progress.finish_error(exit_code, &last_lines);
                return Err(IsolaError::ProvisionFailed(exit_code));
            }

            // Cache the layer
            if layer_name == "base" {
                progress.start_step("Caching base layer...");
                match rootfs::cache_base_layer(name, shell) {
                    Ok(_) => progress.finish_step("Cached base layer"),
                    Err(e) => progress.finish_step(&format!("Cache skipped: {e}")),
                }
            } else {
                match rootfs::cache_env_layer(name, layer_name, shell, registry) {
                    Ok(Some(_)) => {
                        progress.finish_step(&format!("Cached {layer_name} layer"));
                    }
                    Ok(None) => {
                        progress.finish_step(&format!("Provisioned {layer_name} (no cache file)"));
                    }
                    Err(e) => {
                        progress.finish_step(&format!("Cache skipped for {layer_name}: {e}"));
                    }
                }
            }
        }
        true
    } else {
        // No cache at all: download base rootfs and do full provision
        let tarball = rootfs::ensure_rootfs_cached_with_progress(&progress)?;
        progress.start_step("Extracting rootfs...");
        rootfs::extract_rootfs(&tarball, &rootfs_path)?;
        progress.finish_step("Extracted rootfs");
        false
    };

    // Configure rootfs (sandbox-specific: hostname, git config, shell config, etc.)
    progress.start_step("Configuring rootfs...");
    rootfs::post_setup_rootfs(&rootfs_path, name, shell, environments, registry)?;
    progress.finish_step("Configured rootfs");

    // Save config (may already exist from partial layer build, save again to ensure latest)
    save_config(name, &workspace, environments, share_ssh, shell)?;

    if used_layered_cache {
        // Layered path: fix ownership + set up PATH
        progress.start_step("Fixing ownership...");
        let script = rootfs::build_layered_fixup_script(environments, registry);
        let exit_code = crate::commands::enter::run_command(name, &script)?;
        if exit_code != 0 {
            return Err(IsolaError::ProvisionFailed(exit_code));
        }
        progress.finish_step("Ownership fixed");

        if let Some(ref status) = layer_status {
            let cached_names: Vec<String> = status.cached.iter().map(|(n, _)| n.clone()).collect();
            progress.finish_layered(environments, &cached_names, &status.uncached);
        } else {
            progress.finish_cached(environments);
        }
    } else if !no_cache && rootfs::has_cached_provision(environments, shell.name()).is_some() {
        // Legacy monolithic cache was used
        progress.start_step("Fixing ownership...");
        let script = rootfs::build_fixup_script();
        let exit_code = crate::commands::enter::run_command(name, &script)?;
        if exit_code != 0 {
            return Err(IsolaError::ProvisionFailed(exit_code));
        }
        progress.finish_step("Ownership fixed");
        progress.finish_cached(environments);
    } else {
        // Full provisioning (no cache hit at all)
        progress.start_provision();
        let script = rootfs::build_provision_script(environments, shell, registry);
        let child = crate::commands::enter::run_command_captured(name, &script)?;
        let (exit_code, last_lines) =
            progress::monitor_provisioning(child, &progress, environments)?;

        if exit_code != 0 {
            progress.finish_error(exit_code, &last_lines);
            return Err(IsolaError::ProvisionFailed(exit_code));
        }

        // Cache as layers for future use
        progress.start_step("Caching layers...");
        let mut cached_any = false;

        match rootfs::cache_base_layer(name, shell) {
            Ok(_) => cached_any = true,
            Err(e) => eprintln!("  Warning: base layer cache failed: {e}"),
        }

        match rootfs::cache_provisioned_rootfs(name, environments, shell.name()) {
            Ok(()) => cached_any = true,
            Err(e) => eprintln!("  Warning: monolithic cache failed: {e}"),
        }

        if cached_any {
            progress.finish_step("Cached for future use");
        } else {
            progress.finish_step("Cache skipped");
        }

        progress.finish_success(environments);
    }

    Ok(())
}

fn save_config(
    name: &str,
    workspace: &Option<PathBuf>,
    environments: &[String],
    share_ssh: bool,
    shell: &SandboxShell,
) -> Result<(), IsolaError> {
    let config = SandboxConfig {
        name: name.to_string(),
        created_at: Utc::now(),
        rootfs_url: rootfs::rootfs_url().to_string(),
        workspace: workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p)),
        environments: environments.to_vec(),
        share_ssh,
        shell: shell.clone(),
    };
    config.save()
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
