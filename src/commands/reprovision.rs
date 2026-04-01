use crate::error::IsolaError;
use crate::paths;
use crate::plugin::PluginRegistry;
use crate::progress::{self, CreationProgress};
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::rootfs;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    if !rootfs_path.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let environments = &config.environments;
    let registry = PluginRegistry::load()?;

    let progress = CreationProgress::new(&format!("Re-provisioning '{name}'"));

    progress.start_step("Configuring rootfs...");
    rootfs::post_setup_rootfs(&rootfs_path, name, &config.shell, environments, &registry)?;
    progress.finish_step("Configured rootfs");

    // Check which layers need rebuilding
    let layer_status = rootfs::check_layer_cache(environments, &config.shell, &registry);
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
            rootfs::build_env_layer_script(layer_name, &registry).ok_or_else(|| {
                IsolaError::PluginError(format!("no plugin found for '{layer_name}'"))
            })?
        };

        progress.start_step(&format!("Provisioning {layer_name}..."));
        let child = crate::commands::enter::run_command_captured(name, &script)?;
        let (exit_code, last_lines) =
            progress::monitor_provisioning(child, &progress, std::slice::from_ref(layer_name))?;
        if exit_code != 0 {
            progress.finish_error(exit_code, &last_lines);
            return Err(IsolaError::ProvisionFailed(exit_code));
        }

        // Cache the layer
        if layer_name == "base" {
            if let Err(e) = rootfs::cache_base_layer(name, &config.shell) {
                eprintln!("warning: failed to cache base layer: {e}");
            }
        } else if let Err(e) = rootfs::cache_env_layer(name, layer_name, &config.shell, &registry) {
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
