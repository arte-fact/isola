mod common;

/// Full sandbox lifecycle: create → exec → destroy.
///
/// Requires user namespace support and network access (downloads rootfs).
/// Run with: cargo test -- --ignored
#[test]
#[ignore]
fn create_exec_destroy() {
    skip_without_userns!();

    let name = common::unique_sandbox_name("lifecycle");

    // Create sandbox with no environments (fastest)
    let create = common::isola(&["create", &name, "--no-cache"])
        .output()
        .expect("failed to run isola create");

    if !create.status.success() {
        let stderr = String::from_utf8_lossy(&create.stderr);
        panic!("isola create failed: {stderr}");
    }

    // Exec a simple command inside the sandbox
    let exec = common::isola(&["exec", &name, "--", "echo", "hello from sandbox"])
        .output()
        .expect("failed to run isola exec");

    assert!(
        exec.status.success(),
        "isola exec failed: {}",
        String::from_utf8_lossy(&exec.stderr)
    );
    let stdout = String::from_utf8_lossy(&exec.stdout);
    assert!(
        stdout.contains("hello from sandbox"),
        "expected output not found, got: {stdout}"
    );

    // Destroy the sandbox
    let destroy = common::isola(&["destroy", &name])
        .output()
        .expect("failed to run isola destroy");

    assert!(
        destroy.status.success(),
        "isola destroy failed: {}",
        String::from_utf8_lossy(&destroy.stderr)
    );

    // Verify sandbox directory is gone
    let home = std::env::var("HOME").unwrap();
    let sandbox_dir = std::path::Path::new(&home)
        .join(".isola")
        .join("sandboxes")
        .join(&name);
    assert!(
        !sandbox_dir.exists(),
        "sandbox directory still exists after destroy: {}",
        sandbox_dir.display()
    );
}

/// Verify that creating a sandbox with a duplicate name fails.
#[test]
#[ignore]
fn create_duplicate_name_fails() {
    skip_without_userns!();

    let name = common::unique_sandbox_name("dup");

    // Create first sandbox
    let create1 = common::isola(&["create", &name, "--no-cache"])
        .output()
        .expect("failed to run isola create");

    if !create1.status.success() {
        let stderr = String::from_utf8_lossy(&create1.stderr);
        panic!("first isola create failed: {stderr}");
    }

    // Try to create again with same name — should fail
    let create2 = common::isola(&["create", &name, "--no-cache"])
        .output()
        .expect("failed to run isola create");

    assert!(
        !create2.status.success(),
        "duplicate create should have failed"
    );
    let stderr = String::from_utf8_lossy(&create2.stderr);
    assert!(
        stderr.contains("already exists"),
        "expected 'already exists' error, got: {stderr}"
    );

    // Cleanup
    let _ = common::isola(&["destroy", &name]).output();
}

/// Verify that destroying a nonexistent sandbox fails gracefully.
#[test]
fn destroy_nonexistent_fails() {
    let destroy = common::isola(&["destroy", "this-sandbox-does-not-exist-12345"])
        .output()
        .expect("failed to run isola destroy");

    assert!(
        !destroy.status.success(),
        "destroying nonexistent sandbox should fail"
    );
    let stderr = String::from_utf8_lossy(&destroy.stderr);
    assert!(
        stderr.contains("not found"),
        "expected 'not found' error, got: {stderr}"
    );
}

/// Verify that status for a nonexistent sandbox fails gracefully.
#[test]
fn status_nonexistent_fails() {
    let status = common::isola(&["status", "this-sandbox-does-not-exist-12345"])
        .output()
        .expect("failed to run isola status");

    assert!(
        !status.status.success(),
        "status of nonexistent sandbox should fail"
    );
}
