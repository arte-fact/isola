use std::path::{Path, PathBuf};

use nix::mount::{MsFlags, mount};

use crate::error::IsolaError;

fn do_mount(
    label: &str,
    source: Option<&str>,
    target: &Path,
    fstype: Option<&str>,
    flags: MsFlags,
    data: Option<&str>,
) -> Result<(), IsolaError> {
    mount(source, target, fstype, flags, data).map_err(|e| {
        IsolaError::NamespaceError(format!(
            "mount '{label}' on {} failed: {e}",
            target.display()
        ))
    })
}

pub fn setup_mounts(
    rootfs: &Path,
    workspace_host: Option<&Path>,
    host_mounts: &[(String, String, bool)],
    share_display: bool,
) -> Result<(), IsolaError> {
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

    // 9. Mount /dev/pts (try gid=5 first, fallback to gid=0 for user namespaces)
    let pts_path = dev_path.join("pts");
    std::fs::create_dir_all(&pts_path)?;
    let devpts_flags = MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC;
    if mount(
        Some("devpts"),
        &pts_path,
        Some("devpts"),
        devpts_flags,
        Some("newinstance,ptmxmode=0666,mode=0620,gid=5"),
    )
    .is_err()
    {
        let _ = mount(
            Some("devpts"),
            &pts_path,
            Some("devpts"),
            devpts_flags,
            Some("newinstance,ptmxmode=0666,mode=0620,gid=0"),
        );
    }

    // Create /dev/ptmx symlink (needed by posix_openpt)
    let _ = std::os::unix::fs::symlink("pts/ptmx", dev_path.join("ptmx"));

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

    // 11. Bind-mount workspace (if provided), using the actual directory name
    if let Some(ws) = workspace_host {
        let dir_name = ws
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");
        let ws_target = rootfs.join(dir_name);
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

    // 12. Bind-mount /etc/resolv.conf for live DNS
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

    // 13. Plugin-declared host directory bind-mounts (host_mount in plugin.yaml)
    let home = std::env::var("HOME").unwrap_or_default();
    for (from, to, readonly) in host_mounts {
        let src = PathBuf::from(&home).join(from);
        if !src.exists() {
            continue;
        }
        let target = rootfs.join("home/sandbox").join(to);
        if src.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::File::create(&target)?;
        }
        do_mount(
            &format!("host_mount {from}"),
            Some(&src.to_string_lossy()),
            &target,
            none,
            MsFlags::MS_BIND | MsFlags::MS_REC,
            none,
        )?;
        if *readonly
            && let Err(e) = mount(
                none,
                &target,
                none,
                MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY | MsFlags::MS_REC,
                none,
            )
        {
            eprintln!("warning: could not remount {from} read-only: {e}");
        }
    }

    // 14. Display sharing (X11/Wayland) when requested
    if share_display {
        // X11: bind-mount /tmp/.X11-unix into the tmpfs already mounted at tmp_path
        if Path::new("/tmp/.X11-unix").exists() {
            let x11_target = tmp_path.join(".X11-unix");
            std::fs::create_dir_all(&x11_target)?;
            do_mount(
                "X11 sockets",
                Some("/tmp/.X11-unix"),
                &x11_target,
                none,
                MsFlags::MS_BIND | MsFlags::MS_REC,
                none,
            )?;
        }

        // Wayland: bind-mount socket file into /run/user/1000/
        let host_xdg =
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());
        if let Ok(wayland) = std::env::var("WAYLAND_DISPLAY") {
            let host_sock = PathBuf::from(&host_xdg).join(&wayland);
            if host_sock.exists() {
                let sandbox_sock = rootfs.join("run/user/1000").join(&wayland);
                std::fs::File::create(&sandbox_sock)?;
                do_mount(
                    "wayland socket",
                    Some(&host_sock.to_string_lossy()),
                    &sandbox_sock,
                    none,
                    MsFlags::MS_BIND,
                    none,
                )?;
            }
        }

        // Xauthority: bind-mount to /home/sandbox/.Xauthority
        let xauth = std::env::var("XAUTHORITY").unwrap_or_else(|_| {
            format!("{}/.Xauthority", std::env::var("HOME").unwrap_or_default())
        });
        let xauth_path = PathBuf::from(&xauth);
        if xauth_path.exists() {
            let sandbox_xauth = rootfs.join("home/sandbox/.Xauthority");
            std::fs::File::create(&sandbox_xauth)?;
            do_mount(
                "Xauthority",
                Some(&xauth_path.to_string_lossy()),
                &sandbox_xauth,
                none,
                MsFlags::MS_BIND,
                none,
            )?;
        }
    }

    Ok(())
}
