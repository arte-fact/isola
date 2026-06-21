use std::process::Command;

/// Check if user namespaces are available on this system.
/// Returns false if CLONE_NEWUSER is not supported (e.g., in containers without
/// the required capabilities, or when AppArmor restricts unprivileged userns).
pub fn has_userns_support() -> bool {
    // Try to unshare a user namespace via the `unshare` command.
    // This is the most reliable check without using unsafe code.
    Command::new("unshare")
        .args(["--user", "--", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Skip the current test if user namespace support is unavailable.
/// Call this at the start of any integration test that requires namespaces.
#[macro_export]
macro_rules! skip_without_userns {
    () => {
        if !common::has_userns_support() {
            eprintln!("SKIPPED: user namespaces not available in this environment");
            return;
        }
    };
}

/// Generate a unique sandbox name for a test to avoid collisions.
pub fn unique_sandbox_name(prefix: &str) -> String {
    format!(
        "test-{}-{}-{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    )
}

/// Run the isola binary with the given arguments.
pub fn isola(args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_isola"));
    cmd.args(args);
    cmd
}
