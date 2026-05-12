use crate::error::IsolaError;
use crate::paths;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    if !sandbox_dir.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }
    crate::backend::destroy_sandbox(name)
}
