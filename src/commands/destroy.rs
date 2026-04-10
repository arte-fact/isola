use crate::commands::enter::build_env_vars;
use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::namespace::{SandboxExec, enter_sandbox};

pub fn run(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    if !sandbox_dir.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    // Enter the full sandbox namespace (user + mount + pid) and rm -rf from
    // inside.  After pivot_root the process is root with full control over the
    // sandbox filesystem, so it can delete files owned by any subordinate UID.
    // Skip mount points (proc, sys, dev, tmp, etc.) — they can't be removed
    // while mounted and the host-side remove_dir_all handles them after the
    // namespace exits.
    let rootfs = paths::rootfs_dir(name);
    if rootfs.exists() {
        let exec = SandboxExec {
            rootfs: rootfs.to_string_lossy().to_string(),
            exec_path: "/bin/sh".to_string(),
            exec_args: vec![
                "sh".to_string(),
                "-c".to_string(),
                // Delete everything except mount points; silence errors from busy mounts
                "find / -mindepth 1 -maxdepth 1 \
                    ! -name proc ! -name sys ! -name dev ! -name tmp \
                    ! -name workspace ! -name .old_root \
                    -exec rm -rf {} + 2>/dev/null; true"
                    .to_string(),
            ],
            env_vars: build_env_vars(false),
            workspace_host: None,

            host_mounts: vec![],
            share_display: false,
            run_as_uid: None,
            multi_uid: true,
            capture_output: false,
            devices: vec![],
        };
        if let Err(e) = enter_sandbox(exec) {
            eprintln!("warning: in-sandbox cleanup failed: {e}");
            eprintln!("  (continuing with host-side deletion)");
        }
    }

    // Now delete the sandbox directory from the host.  Most contents were
    // removed above; only empty dirs and host-owned files (config.json) remain.
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
