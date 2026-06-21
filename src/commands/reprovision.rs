use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    if !sandbox_dir.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;

    #[cfg(target_os = "linux")]
    {
        let environments = &config.environments;
        run_linux(name, &config, environments)
    }

    #[cfg(target_os = "macos")]
    {
        run_macos(name, &config.environments, &config.plugin_vars)
    }
}

#[cfg(target_os = "macos")]
fn run_macos(
    name: &str,
    environments: &[String],
    plugin_vars: &std::collections::BTreeMap<String, String>,
) -> Result<(), IsolaError> {
    use crate::sandbox::backend;
    let b = backend::create_backend();
    b.write_sandbox_files(name, environments)?;

    let script = b.build_provision_script(environments, plugin_vars);
    eprintln!("Re-provisioning '{}': {}...", name, environments.join(", "));
    let exit_code = b.run_command(name, &script)?;
    if exit_code != 0 {
        return Err(IsolaError::ProvisionFailed(exit_code));
    }

    eprintln!("Sandbox '{}' re-provisioned successfully!", name);
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_linux(
    name: &str,
    config: &SandboxConfig,
    environments: &[String],
) -> Result<(), IsolaError> {
    use crate::plugin::PluginRegistry;
    use crate::progress::{self, CreationProgress};
    use crate::sandbox::rootfs;

    let registry = PluginRegistry::load()?;
    let rootfs_path = paths::rootfs_dir(name);
    let progress = CreationProgress::new(&format!("Re-provisioning '{name}'"));

    progress.start_step("Configuring rootfs...");
    rootfs::post_setup_rootfs(&rootfs_path, name, environments, &registry, false)?;
    progress.finish_step("Configured rootfs");

    // Check which layers need rebuilding
    let layer_status =
        rootfs::check_layer_cache(environments, &config.shell, &registry, &config.plugin_vars);
    let mut built_layers = Vec::new();
    let mut cached_layers = Vec::new();

    // Extract cached layers first
    if !layer_status.cached.is_empty() {
        progress.start_step("Extracting cached layers...");
        for (layer_name, layer_path) in &layer_status.cached {
            rootfs::extract_rootfs(layer_path, &rootfs_path)?;
            cached_layers.push(layer_name.clone());
        }
        progress.finish_step("Extracted cached layers");
    }

    // Build uncached layers
    for layer_name in &layer_status.uncached {
        let script = if layer_name == "base" {
            rootfs::build_base_layer_script(&config.shell)
        } else {
            rootfs::build_env_layer_script(layer_name, &registry, &config.plugin_vars).ok_or_else(
                || IsolaError::PluginError(format!("no plugin found for '{layer_name}'")),
            )?
        };

        progress.start_step(&format!("Provisioning {layer_name}..."));
        let child = crate::commands::enter::run_command_captured(name, &script)?;
        let (exit_code, last_lines) = progress::monitor_provisioning(child, &progress, &script)?;
        if exit_code != 0 {
            progress.finish_error(exit_code, &last_lines);
            return Err(IsolaError::ProvisionFailed(exit_code));
        }

        // Cache the layer. For the base layer the tarball is produced inside
        // the sandbox (so unmapped subuids on the host don't cause EACCES);
        // env layers tar themselves as part of their provision script.
        if layer_name == "base" {
            let res = (|| -> Result<(), IsolaError> {
                let cache_script = rootfs::build_base_cache_script();
                let child = crate::commands::enter::run_command_captured(name, &cache_script)?;
                let (exit_code, _) =
                    progress::monitor_provisioning(child, &progress, &cache_script)?;
                if exit_code != 0 {
                    return Err(IsolaError::ProvisionFailed(exit_code));
                }
                rootfs::cache_base_layer(name, &config.shell).map(|_| ())
            })();
            if let Err(e) = res {
                eprintln!("warning: failed to cache base layer: {e}");
            }
        } else if let Err(e) = rootfs::cache_env_layer(
            name,
            layer_name,
            &config.shell,
            &registry,
            &config.plugin_vars,
        ) {
            eprintln!("warning: failed to cache {layer_name} layer: {e}");
        }

        built_layers.push(layer_name.clone());
    }

    // Fixup PATH + ownership
    progress.start_step("Fixing ownership...");
    let fixup = rootfs::build_layered_fixup_script(environments, &registry);
    let exit_code = crate::commands::enter::run_command(name, &fixup)?;
    if exit_code != 0 {
        return Err(IsolaError::ProvisionFailed(exit_code));
    }
    progress.finish_step("Ownership fixed");

    progress.finish_layered(environments, &cached_layers, &built_layers);
    Ok(())
}
