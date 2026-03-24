use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::userns;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    if !sandbox_dir.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    // Files may be owned by subordinate UIDs. Use a lightweight user namespace
    // to chown everything back to our host UID before deleting.
    let rootfs = paths::rootfs_dir(name);
    if rootfs.exists() {
        let rootfs_str = rootfs.to_string_lossy().to_string();
        let _ = userns::run_in_userns(
            move || {
                let _ = std::process::Command::new("chown")
                    .args(["-R", "0:0", &rootfs_str])
                    .status();
                0
            },
            false,
        );
    }

    match std::fs::remove_dir_all(&sandbox_dir) {
        Ok(()) => {
            eprintln!("Sandbox '{}' destroyed", name);
            Ok(())
        }
        Err(e) => {
            eprintln!("Warning: some files could not be deleted: {e}");
            eprintln!("Try: rm -rf {}", sandbox_dir.display());
            Err(IsolaError::Io(e))
        }
    }
}
