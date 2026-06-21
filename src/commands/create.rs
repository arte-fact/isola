use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::Utc;

#[cfg(target_os = "linux")]
use crate::error::IoContext;
use crate::error::IsolaError;
use crate::paths;
use crate::plugin::{PluginLayer, PluginRegistry};
use crate::sandbox::backend;
use crate::sandbox::config::{SandboxConfig, SandboxShell};

/// Create a sandbox with all project-layer plugins plus auto-detected user-layer plugins (CLI shorthand).
/// If `plugins` is non-empty, those plugins are installed instead of all project-layer plugins.
pub fn run(
    name: &str,
    workspace: Option<PathBuf>,
    no_cache: bool,
    plugins: Vec<String>,
) -> Result<(), IsolaError> {
    let registry = PluginRegistry::load()?;
    let home = std::env::var("HOME").ok().map(PathBuf::from);

    let mut envs: Vec<String> = if plugins.is_empty() {
        registry
            .plugins_for_layer(PluginLayer::Project)
            .into_iter()
            .map(|p| p.manifest.name.clone())
            .collect()
    } else {
        registry.validate_environments(&plugins)?;
        plugins
    };

    // Auto-add user-layer plugins whose host path is detected (e.g. claude-config, ssh-keys)
    for p in registry.plugins_for_layer(PluginLayer::User) {
        if let Some(ref ad) = p.manifest.auto_detect
            && home
                .as_ref()
                .map(|h| h.join(&ad.host_path).exists())
                .unwrap_or(false)
        {
            envs.push(p.manifest.name.clone());
        }
    }

    run_with_envs(CreateRequest {
        name,
        workspace,
        environments: &envs,
        share_display: false,
        no_cache,
        shell: &SandboxShell::default(),
        registry: &registry,
        plugin_vars: &BTreeMap::new(),
    })
}

/// Create a sandbox with selected environments
#[allow(clippy::too_many_arguments)]
/// Everything needed to create a sandbox, bundled so the platform-specific
/// create functions take a single argument instead of a long parameter list.
pub struct CreateRequest<'a> {
    pub name: &'a str,
    pub workspace: Option<PathBuf>,
    pub environments: &'a [String],
    pub share_display: bool,
    pub no_cache: bool,
    pub shell: &'a SandboxShell,
    pub registry: &'a PluginRegistry,
    pub plugin_vars: &'a BTreeMap<String, String>,
}

pub fn run_with_envs(req: CreateRequest) -> Result<(), IsolaError> {
    validate_name(req.name)?;

    let b = backend::create_backend();
    b.preflight_checks()?;

    let sandbox_dir = paths::sandbox_dir(req.name);
    if sandbox_dir.exists() {
        return Err(IsolaError::SandboxExists(req.name.to_string()));
    }

    #[cfg(target_os = "linux")]
    {
        run_linux(req)
    }

    #[cfg(target_os = "macos")]
    {
        run_macos(req)
    }
}

#[cfg(target_os = "macos")]
fn run_macos(req: CreateRequest) -> Result<(), IsolaError> {
    let CreateRequest {
        name,
        workspace,
        environments,
        share_display,
        shell,
        plugin_vars,
        ..
    } = req;
    let b = backend::create_backend();

    let workspace = workspace
        .or_else(|| std::env::current_dir().ok())
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p));

    b.create_environment(name, workspace.as_deref())?;
    b.write_sandbox_files(name, environments)?;

    let config = SandboxConfig {
        name: name.to_string(),
        created_at: Utc::now(),
        rootfs_url: b.rootfs_url().to_string(),
        workspace,
        environments: environments.to_vec(),
        share_display,
        shell: shell.clone(),
        devices: vec![],
        plugin_vars: plugin_vars.clone(),
    };
    config.save()?;

    eprintln!("Sandbox '{}' created successfully", name);

    let script = b.build_provision_script(environments, plugin_vars);
    eprintln!("Provisioning: {}...", environments.join(", "));
    let exit_code = b.run_command(name, &script)?;
    if exit_code != 0 {
        return Err(IsolaError::ProvisionFailed(exit_code));
    }

    eprintln!("Sandbox '{}' is ready!", name);
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_linux(req: CreateRequest) -> Result<(), IsolaError> {
    let CreateRequest {
        name,
        workspace,
        environments,
        share_display,
        no_cache,
        shell,
        registry,
        plugin_vars,
    } = req;
    use crate::progress::{self, CreationProgress};
    use crate::sandbox::rootfs;

    let progress = CreationProgress::new(name);
    progress.finish_step("Preflight checks passed");

    let rootfs_path = paths::rootfs_dir(name);
    std::fs::create_dir_all(&rootfs_path).io_ctx("create sandbox rootfs dir", &rootfs_path)?;

    // Try layered cache first, then legacy monolithic cache, then full provision
    let layer_status = if no_cache {
        None
    } else {
        Some(rootfs::check_layer_cache(
            environments,
            shell,
            registry,
            plugin_vars,
        ))
    };

    // `used_layered_cache` drives the post-extract ownership/PATH fixup;
    // `from_cache` is true whenever the rootfs was extracted from any cache
    // (so it already contains provisioned, possibly read-only files) — in that
    // case post_setup_rootfs must run in tolerant mode.
    let (used_layered_cache, from_cache) = if let Some(ref status) = layer_status
        && status.all_cached()
    {
        // Fast path: all layers cached
        progress.start_step("Extracting cached layers...");
        for (_, layer_path) in &status.cached {
            rootfs::extract_rootfs(layer_path, &rootfs_path)?;
        }
        rootfs::ensure_rootfs_has_bash(&rootfs_path)?;
        progress.finish_step("Extracted cached layers");
        (true, true)
    } else if !no_cache
        && let Some(cache_path) = rootfs::has_cached_provision(environments, shell.name())
    {
        // Legacy fallback: monolithic cache exists
        progress.start_step("Extracting cached rootfs...");
        rootfs::extract_rootfs(&cache_path, &rootfs_path)?;
        rootfs::ensure_rootfs_has_bash(&rootfs_path)?;
        progress.finish_step("Extracted cached rootfs");
        (false, true)
    } else if let Some(ref status) = layer_status
        && !status.cached.is_empty()
    {
        // Partial layered cache: extract cached layers, build missing ones
        progress.start_step("Extracting cached layers...");
        for (_, layer_path) in &status.cached {
            rootfs::extract_rootfs(layer_path, &rootfs_path)?;
        }
        // Must check before the uncached loop — run_command_captured execs
        // /bin/bash, which surfaces a corrupt cache as a cryptic ENOENT.
        rootfs::ensure_rootfs_has_bash(&rootfs_path)?;
        progress.finish_step("Extracted cached layers");

        // Build (and cache) each uncached layer.
        for layer_name in &status.uncached {
            let script = if layer_name == "base" {
                rootfs::build_base_layer_script(shell)
            } else {
                rootfs::build_env_layer_script(layer_name, registry, plugin_vars).ok_or_else(
                    || IsolaError::PluginError(format!("no plugin found for '{layer_name}'")),
                )?
            };

            progress.start_step(&format!("Provisioning {layer_name}..."));
            // Save config early so run_command_captured can find the sandbox.
            save_config(
                name,
                &workspace,
                environments,
                share_display,
                shell,
                plugin_vars,
            )?;

            provision_one_layer(name, &script, &progress)?;
            cache_one_layer(name, layer_name, shell, registry, plugin_vars, &progress);
        }
        // Some layers were extracted from cache; base/env layers don't carry
        // host_copy configs, but treat it as cache-derived to be safe.
        (true, true)
    } else {
        // No cache at all: download base rootfs and do full provision
        let tarball = rootfs::ensure_rootfs_cached_with_progress(&progress)?;
        progress.start_step("Extracting rootfs...");
        rootfs::extract_rootfs(&tarball, &rootfs_path)?;
        rootfs::ensure_rootfs_has_bash(&rootfs_path)?;
        progress.finish_step("Extracted rootfs");
        (false, false)
    };

    // Configure rootfs (sandbox-specific: hostname, git config, shell config, etc.)
    // On a cache hit the rootfs already contains provisioned (possibly
    // read-only) files, so run in tolerant mode.
    progress.start_step("Configuring rootfs...");
    rootfs::post_setup_rootfs(&rootfs_path, name, environments, registry, !from_cache)?;
    progress.finish_step("Configured rootfs");

    // Save config (may already exist from partial layer build, save again to ensure latest)
    save_config(
        name,
        &workspace,
        environments,
        share_display,
        shell,
        plugin_vars,
    )?;

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
        let script = rootfs::build_provision_script(environments, shell, registry, plugin_vars);
        let child = crate::commands::enter::run_command_captured(name, &script)?;
        let (exit_code, last_lines) = progress::monitor_provisioning(child, &progress, &script)?;

        if exit_code != 0 {
            progress.finish_error(exit_code, &last_lines);
            return Err(IsolaError::ProvisionFailed(exit_code));
        }

        // Cache the whole provisioned rootfs for an exact-config rerun. The
        // tarball is produced *inside* the sandbox so tar can read files owned
        // by the mapped sandbox UID (an unmapped subuid from the host's POV).
        // We deliberately do NOT call cache_base_layer here: the post-provision
        // rootfs has env layers mixed in, so captured as "base" it would
        // poison future sandboxes that match the base-layer hash but expect
        // a pure base. Layered caches are built only via the layered path.
        progress.start_step("Caching for future use...");
        let cache_result = run_cache_script_and_move(
            name,
            &rootfs::build_provision_cache_script(),
            &progress,
            || rootfs::cache_provisioned_rootfs(name, environments, shell.name()),
        );
        match cache_result {
            Ok(()) => progress.finish_step("Cached for future use"),
            Err(e) => {
                eprintln!("  Warning: cache failed: {e}");
                progress.finish_step("Cache skipped");
            }
        }

        progress.finish_success(environments);
    }

    Ok(())
}

/// Run one layer's provisioning script in the sandbox, surfacing failures.
#[cfg(target_os = "linux")]
fn provision_one_layer(
    name: &str,
    script: &str,
    progress: &crate::progress::CreationProgress,
) -> Result<(), IsolaError> {
    let child = crate::commands::enter::run_command_captured(name, script)?;
    let (exit_code, last_lines) = crate::progress::monitor_provisioning(child, progress, script)?;
    if exit_code != 0 {
        progress.finish_error(exit_code, &last_lines);
        return Err(IsolaError::ProvisionFailed(exit_code));
    }
    Ok(())
}

/// Capture a freshly provisioned layer into the layer cache (best-effort: a
/// cache failure is reported via progress but never fails the create).
#[cfg(target_os = "linux")]
fn cache_one_layer(
    name: &str,
    layer_name: &str,
    shell: &SandboxShell,
    registry: &PluginRegistry,
    plugin_vars: &BTreeMap<String, String>,
    progress: &crate::progress::CreationProgress,
) {
    use crate::sandbox::rootfs;
    if layer_name == "base" {
        progress.start_step("Caching base layer...");
        let res =
            run_cache_script_and_move(name, &rootfs::build_base_cache_script(), progress, || {
                rootfs::cache_base_layer(name, shell).map(|_| ())
            });
        match res {
            Ok(()) => progress.finish_step("Cached base layer"),
            Err(e) => progress.finish_step(&format!("Cache skipped: {e}")),
        }
    } else {
        match rootfs::cache_env_layer(name, layer_name, shell, registry, plugin_vars) {
            Ok(Some(_)) => progress.finish_step(&format!("Cached {layer_name} layer")),
            Ok(None) => progress.finish_step(&format!("Provisioned {layer_name} (no cache file)")),
            Err(e) => progress.finish_step(&format!("Cache skipped for {layer_name}: {e}")),
        }
    }
}

#[cfg(target_os = "linux")]
fn run_cache_script_and_move(
    name: &str,
    script: &str,
    progress: &crate::progress::CreationProgress,
    move_out: impl FnOnce() -> Result<(), IsolaError>,
) -> Result<(), IsolaError> {
    let child = crate::commands::enter::run_command_captured(name, script)?;
    let (exit_code, _) = crate::progress::monitor_provisioning(child, progress, script)?;
    if exit_code != 0 {
        return Err(IsolaError::ProvisionFailed(exit_code));
    }
    move_out()
}

#[cfg(target_os = "linux")]
fn save_config(
    name: &str,
    workspace: &Option<PathBuf>,
    environments: &[String],
    share_display: bool,
    shell: &SandboxShell,
    plugin_vars: &BTreeMap<String, String>,
) -> Result<(), IsolaError> {
    use crate::sandbox::rootfs;

    let config = SandboxConfig {
        name: name.to_string(),
        created_at: Utc::now(),
        rootfs_url: rootfs::rootfs_url().to_string(),
        workspace: workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p)),
        environments: environments.to_vec(),
        share_display,
        shell: shell.clone(),
        devices: vec![],
        plugin_vars: plugin_vars.clone(),
    };
    config.save()
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
