mod common;

use std::path::PathBuf;

/// Run a shell snippet inside a sandbox and return trimmed stdout, asserting
/// success.
fn exec_out(name: &str, script: &str) -> String {
    let out = common::isola(&["exec", name, "--", "/bin/bash", "-lc", script])
        .output()
        .expect("failed to run isola exec");
    assert!(
        out.status.success(),
        "exec `{script}` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn create_minimal(name: &str, workspace: &std::path::Path) {
    let create = common::isola(&[
        "create",
        name,
        "--plugins",
        "git",
        "-w",
        workspace.to_str().unwrap(),
    ])
    .output()
    .expect("failed to run isola create");
    assert!(
        create.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&create.stderr)
    );
}

/// End-to-end isolation invariants: identity, namespaces, seccomp, sudo,
/// and workspace write-back. Heavy (downloads/provisions) — run with
/// `cargo test -- --ignored`.
#[test]
#[ignore]
fn sandbox_isolation_invariants() {
    skip_without_userns!();

    let name = common::unique_sandbox_name("iso");
    let ws_name = format!("{name}-ws");
    let ws: PathBuf = std::env::temp_dir().join(&ws_name);
    std::fs::create_dir_all(&ws).unwrap();

    create_minimal(&name, &ws);

    // Unprivileged sandbox user.
    assert_eq!(exec_out(&name, "id -u"), "1000");
    // UTS namespace: hostname is the sandbox name.
    assert_eq!(exec_out(&name, "hostname"), name);
    // seccomp filter is active (mode 2 = filter).
    assert_eq!(
        exec_out(&name, "grep -oP 'Seccomp:\\s*\\K[0-9]' /proc/self/status"),
        "2"
    );
    // PID namespace: only a handful of processes are visible.
    let nproc: usize = exec_out(&name, "ls /proc | grep -cE '^[0-9]+$'")
        .parse()
        .unwrap();
    assert!(
        nproc < 25,
        "expected an isolated PID namespace, saw {nproc}"
    );
    // sudo still elevates under seccomp (no_new_privs is intentionally not set).
    assert_eq!(
        exec_out(&name, "echo sandbox | sudo -S id -u 2>/dev/null"),
        "0"
    );

    // A blocked syscall returns EPERM (errno 1); the C toolchain is from base.
    let probe = r#"cat > /tmp/probe.c <<'EOF'
#include <errno.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <stdio.h>
int main(){ errno=0; syscall(SYS_keyctl, 0, -3, 0); printf("%d\n", errno); return 0; }
EOF
gcc -o /tmp/probe /tmp/probe.c && /tmp/probe"#;
    assert_eq!(
        exec_out(&name, probe),
        "1",
        "keyctl must be blocked (EPERM)"
    );

    // Workspace write-back: a file written inside appears on the host, owned by
    // the host user (not a subordinate UID).
    exec_out(&name, &format!("echo hello > /{ws_name}/probe.txt"));
    let host_file = ws.join("probe.txt");
    assert!(host_file.exists(), "workspace write did not reach the host");
    assert_eq!(std::fs::read_to_string(&host_file).unwrap().trim(), "hello");
    let meta = std::fs::metadata(&host_file).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        meta.uid(),
        nix_getuid(),
        "workspace file should be owned by the host user"
    );

    let _ = common::isola(&["destroy", &name]).output();
    let _ = std::fs::remove_dir_all(&ws);
}

/// Regression: reprovisioning a sandbox whose rootfs already contains files
/// owned by mapped subordinate UIDs must not fail with EACCES on host-side
/// config writes.
#[test]
#[ignore]
fn reprovision_after_provisioning_succeeds() {
    skip_without_userns!();

    let name = common::unique_sandbox_name("reprov");
    let ws = std::env::temp_dir().join(format!("{name}-ws"));
    std::fs::create_dir_all(&ws).unwrap();

    create_minimal(&name, &ws);

    let re = common::isola(&["reprovision", &name])
        .output()
        .expect("failed to run isola reprovision");
    assert!(
        re.status.success(),
        "reprovision failed: {}",
        String::from_utf8_lossy(&re.stderr)
    );

    let _ = common::isola(&["destroy", &name]).output();
    let _ = std::fs::remove_dir_all(&ws);
}

fn nix_getuid() -> u32 {
    // Avoid pulling nix into the test crate just for getuid.
    unsafe { libc_getuid() }
}

unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}
