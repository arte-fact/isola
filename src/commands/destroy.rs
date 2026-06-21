use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::backend;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    if !sandbox_dir.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let b = backend::create_backend();
    b.destroy(name)
}
