use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::backend;
use crate::sandbox::config::SandboxConfig;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    if !sandbox_dir.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let environments = &config.environments;

    let b = backend::create_backend();
    b.write_sandbox_files(name, environments)?;

    let script = b.build_provision_script(environments);
    eprintln!("Re-provisioning '{}': {}...", name, environments.join(", "));
    let exit_code = b.run_command(name, &script)?;
    if exit_code != 0 {
        return Err(IsolaError::ProvisionFailed(exit_code));
    }

    eprintln!("Sandbox '{}' re-provisioned successfully!", name);
    Ok(())
}
