use crate::error::IsolaError;
use crate::paths;
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

    let progress = CreationProgress::new(&format!("Re-provisioning '{name}'"));

    progress.start_step("Configuring rootfs...");
    rootfs::post_setup_rootfs(&rootfs_path, name, environments)?;
    progress.finish_step("Configured rootfs");

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
