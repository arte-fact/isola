use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    if !sandbox_dir.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }
    let config = SandboxConfig::load(name)?;
    crate::backend::reprovision(name, &config.environments)
}
