use crate::error::IsolaError;
use crate::paths;
use crate::plugin::PluginRegistry;
use crate::progress::{self, CreationProgress};
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::rootfs;

/// Re-run provisioning inside an existing sandbox (same env set, rootfs kept). The full
/// provision script is idempotent per plugin; the snapshot cache is left untouched — a
/// reprovisioned rootfs has drifted from create-time state and must not overwrite the
/// clean snapshot other creates extract.
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
    rootfs::post_setup_rootfs(&rootfs_path, name, environments, &registry)?;
    progress.finish_step("Configured rootfs");

    progress.start_provision();
    let script = rootfs::build_provision_script(environments, &config.shell, &registry);
    let child = crate::commands::enter::run_command_captured(name, &script)?;
    let (exit_code, last_lines) = progress::monitor_provisioning(child, &progress, &script)?;
    if exit_code != 0 {
        progress.finish_error(exit_code, &last_lines);
        return Err(IsolaError::ProvisionFailed(exit_code));
    }

    progress.start_step("Fixing ownership...");
    let fixup = rootfs::build_fixup_script();
    let exit_code = crate::commands::enter::run_command(name, &fixup)?;
    if exit_code != 0 {
        return Err(IsolaError::ProvisionFailed(exit_code));
    }
    progress.finish_step("Ownership fixed");

    progress.finish_success(environments);
    Ok(())
}
