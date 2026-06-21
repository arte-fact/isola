use isola_core::error::IsolaError;
use isola_core::paths;
use isola_core::sandbox::backend;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    if !sandbox_dir.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let b = backend::create_backend();
    b.destroy(name)
}
