use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::rootfs;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    if !rootfs_path.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let environments = &config.environments;

    rootfs::post_setup_rootfs(&rootfs_path, name, environments)?;

    let script = rootfs::build_provision_script(environments);
    eprintln!("Re-provisioning '{}': {}...", name, environments.join(", "));
    let exit_code = crate::commands::enter::run_command(name, &script)?;
    if exit_code != 0 {
        return Err(IsolaError::ProvisionFailed(exit_code));
    }

    eprintln!("Sandbox '{}' re-provisioned successfully!", name);
    Ok(())
}
