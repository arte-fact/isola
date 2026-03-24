use std::path::Path;

use nix::mount::{MsFlags, mount};

use crate::error::BotError;

fn do_mount(
    label: &str,
    source: Option<&str>,
    target: &Path,
    fstype: Option<&str>,
    flags: MsFlags,
    data: Option<&str>,
) -> Result<(), BotError> {
    mount(source, target, fstype, flags, data).map_err(|e| {
        BotError::NamespaceError(format!(
            "mount '{label}' on {} failed: {e}",
            target.display()
        ))
    })
}

pub fn setup_mounts(
    rootfs: &Path,
    workspace_host: Option<&Path>,
    claude_binary: Option<&Path>,
    session_credentials: Option<&Path>,
) -> Result<(), BotError> {
    let none: Option<&str> = None;

    // 1. Make all mounts slave (prevent propagation to host, allows bind mounts in user NS)
    do_mount(
        "/ slave",
        none,
        Path::new("/"),
        none,
        MsFlags::MS_REC | MsFlags::MS_SLAVE,
        none,
    )?;

    // 2. Bind-mount rootfs onto itself (pivot_root requirement)
    do_mount(
        "rootfs bind",
        Some(&rootfs.to_string_lossy()),
        rootfs,
        none,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        none,
    )?;

    // 3. Mount /proc
    let proc_path = rootfs.join("proc");
    std::fs::create_dir_all(&proc_path)?;
    do_mount(
        "proc",
        Some("proc"),
        &proc_path,
        Some("proc"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
        none,
    )?;

    // 4. Bind-mount /sys read-only from host
    let sys_path = rootfs.join("sys");
    std::fs::create_dir_all(&sys_path)?;
    do_mount(
        "sysfs bind",
        Some("/sys"),
        &sys_path,
        none,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        none,
    )?;
    // Remount read-only (best-effort — may fail in user NS)
    let _ = mount(
        none,
        &sys_path,
        none,
        MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY | MsFlags::MS_REC,
        none,
    );

    // 5. Mount /tmp
    let tmp_path = rootfs.join("tmp");
    std::fs::create_dir_all(&tmp_path)?;
    do_mount(
        "tmpfs /tmp",
        Some("tmpfs"),
        &tmp_path,
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        none,
    )?;

    // 6. Mount /dev (minimal tmpfs)
    let dev_path = rootfs.join("dev");
    std::fs::create_dir_all(&dev_path)?;
    do_mount(
        "tmpfs /dev",
        Some("tmpfs"),
        &dev_path,
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_STRICTATIME,
        Some("mode=755,size=65536k"),
    )?;

    // 7. Bind-mount essential /dev nodes
    for dev in &["null", "zero", "full", "random", "urandom", "tty"] {
        let host_dev = format!("/dev/{dev}");
        let container_dev = dev_path.join(dev);
        std::fs::File::create(&container_dev)?;
        do_mount(
            &format!("/dev/{dev}"),
            Some(&host_dev),
            &container_dev,
            none,
            MsFlags::MS_BIND,
            none,
        )?;
    }

    // 8. Create /dev symlinks
    std::os::unix::fs::symlink("/proc/self/fd", dev_path.join("fd"))?;
    std::os::unix::fs::symlink("/proc/self/fd/0", dev_path.join("stdin"))?;
    std::os::unix::fs::symlink("/proc/self/fd/1", dev_path.join("stdout"))?;
    std::os::unix::fs::symlink("/proc/self/fd/2", dev_path.join("stderr"))?;

    // 9. Mount /dev/pts (best-effort)
    let pts_path = dev_path.join("pts");
    std::fs::create_dir_all(&pts_path)?;
    let _ = mount(
        Some("devpts"),
        &pts_path,
        Some("devpts"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
        Some("newinstance,ptmxmode=0666,mode=0620"),
    );

    // 10. Mount /dev/shm
    let shm_path = dev_path.join("shm");
    std::fs::create_dir_all(&shm_path)?;
    do_mount(
        "tmpfs /dev/shm",
        Some("tmpfs"),
        &shm_path,
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        none,
    )?;

    // 11. Bind-mount Claude binary (if provided)
    if let Some(claude) = claude_binary {
        let claude_target = rootfs.join("usr/local/bin/claude");
        std::fs::create_dir_all(rootfs.join("usr/local/bin"))?;
        std::fs::File::create(&claude_target)?;
        do_mount(
            "claude binary",
            Some(&claude.to_string_lossy()),
            &claude_target,
            none,
            MsFlags::MS_BIND,
            none,
        )?;
    }

    // 12. Bind-mount workspace (if provided)
    if let Some(ws) = workspace_host {
        let ws_target = rootfs.join("workspace");
        std::fs::create_dir_all(&ws_target)?;
        do_mount(
            "workspace",
            Some(&ws.to_string_lossy()),
            &ws_target,
            none,
            MsFlags::MS_BIND,
            none,
        )?;
    }

    // 13. Bind-mount /etc/resolv.conf for live DNS
    let resolv_target = rootfs.join("etc/resolv.conf");
    if resolv_target.exists() {
        do_mount(
            "resolv.conf",
            Some("/etc/resolv.conf"),
            &resolv_target,
            none,
            MsFlags::MS_BIND,
            none,
        )?;
    }

    // 14. Bind-mount shared Claude session credentials
    if let Some(creds) = session_credentials
        && creds.exists()
    {
        let creds_target = rootfs.join("home/sandbox/.claude/.credentials.json");
        if creds_target.exists() {
            do_mount(
                "claude credentials",
                Some(&creds.to_string_lossy()),
                &creds_target,
                none,
                MsFlags::MS_BIND,
                none,
            )?;
        }
    }

    Ok(())
}
