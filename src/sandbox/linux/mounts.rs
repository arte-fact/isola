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
    devices: &[String],
    cache_mounts: &[(String, String)],
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

    // 7b/7c. Pass through host device nodes (GPU, etc.) and NVIDIA driver libs.
    mount_passthrough_devices(rootfs, &dev_path, devices)?;

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
    mount_host_dirs(rootfs, host_mounts)?;

    // 13b. Shared package-manager caches (plugin `cache:` declarations).
    mount_caches(rootfs, cache_mounts)?;

    // 14. Display sharing (X11/Wayland) when requested
    if share_display {
        mount_display(rootfs, &tmp_path)?;
    }

    Ok(())
}

/// Bind-mount passed-through host device nodes (GPU, etc.) into the sandbox /dev.
/// When NVIDIA nodes appear, also bind-mount the host's userspace driver
/// libraries (they must match the running host kernel module exactly, so they
/// come from the host rather than a package; `ldconfig` runs inside the sandbox).
fn mount_passthrough_devices(
    rootfs: &Path,
    dev_path: &Path,
    devices: &[String],
) -> Result<(), IsolaError> {
    let none: Option<&str> = None;
    for device_path in devices {
        let host_dev = Path::new(device_path);
        if !host_dev.exists() {
            continue;
        }
        let relative = host_dev.strip_prefix("/dev/").unwrap_or(host_dev.as_ref());
        let container_dev = dev_path.join(relative);

        if host_dev.is_dir() {
            std::fs::create_dir_all(&container_dev)?;
        } else {
            if let Some(parent) = container_dev.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::File::create(&container_dev)?;
        }

        do_mount(
            &format!("device {device_path}"),
            Some(device_path),
            &container_dev,
            none,
            MsFlags::MS_BIND,
            none,
        )?;
    }

    if devices.iter().any(|d| d.contains("nvidia")) {
        setup_nvidia_libs(rootfs);
    }
    Ok(())
}

/// Bind-mount host directories declared by plugins (`host_mount`) under
/// /home/sandbox, remounting read-only where requested.
fn mount_host_dirs(
    rootfs: &Path,
    host_mounts: &[(String, String, bool)],
) -> Result<(), IsolaError> {
    let none: Option<&str> = None;
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
    Ok(())
}

/// Bind-mount shared package-manager caches at their absolute sandbox paths so
/// build tools reuse downloads across sandboxes. The host directories are
/// created by the backend before clone(); a missing one is skipped silently.
fn mount_caches(rootfs: &Path, cache_mounts: &[(String, String)]) -> Result<(), IsolaError> {
    let none: Option<&str> = None;
    for (host_src, sandbox_dest) in cache_mounts {
        let src = Path::new(host_src);
        if !src.is_dir() {
            continue;
        }
        let rel = sandbox_dest.strip_prefix('/').unwrap_or(sandbox_dest);
        let target = rootfs.join(rel);
        std::fs::create_dir_all(&target)?;
        do_mount(
            &format!("cache {sandbox_dest}"),
            Some(host_src),
            &target,
            none,
            MsFlags::MS_BIND | MsFlags::MS_REC,
            none,
        )?;
    }
    Ok(())
}

/// Share the host display: X11 socket dir, the Wayland socket, and Xauthority.
fn mount_display(rootfs: &Path, tmp_path: &Path) -> Result<(), IsolaError> {
    let none: Option<&str> = None;

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
    let xauth = std::env::var("XAUTHORITY")
        .unwrap_or_else(|_| format!("{}/.Xauthority", std::env::var("HOME").unwrap_or_default()));
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
    Ok(())
}

/// Bind-mount the host's NVIDIA userspace driver libraries and management
/// binaries into the sandbox. Triggered when NVIDIA device nodes are passed
/// through. The driver libraries must match the running host kernel module
/// exactly, so they are bind-mounted straight from the host (the CUDA *toolkit*
/// is installed by the `cuda` plugin; the *driver* is not). Only the real
/// versioned files are mounted — `ldconfig` (run later, inside the sandbox)
/// recreates the soname symlinks and refreshes the loader cache.
///
/// Best-effort: anything missing on the host is skipped. Runs before
/// `pivot_root`, while the host filesystem is still reachable.
fn setup_nvidia_libs(rootfs: &Path) {
    let host_lib_dir = Path::new("/usr/lib/x86_64-linux-gnu");
    let sandbox_lib_dir = rootfs.join("usr/lib/x86_64-linux-gnu");
    let _ = std::fs::create_dir_all(&sandbox_lib_dir);
    if let Ok(entries) = std::fs::read_dir(host_lib_dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let fname = fname.to_string_lossy();
            let is_driver = (fname.starts_with("libnvidia-")
                || fname.starts_with("libcuda.so")
                || fname.starts_with("libnvcuvid.so"))
                && fname.contains(".so");
            if !is_driver {
                continue;
            }
            // Skip symlinks; ldconfig regenerates them from the real files.
            match std::fs::symlink_metadata(entry.path()) {
                Ok(m) if m.file_type().is_symlink() => continue,
                Ok(_) => {}
                Err(_) => continue,
            }
            let target = sandbox_lib_dir.join(fname.as_ref());
            if std::fs::File::create(&target).is_err() {
                continue;
            }
            let _ = do_mount(
                &fname,
                Some(&entry.path().to_string_lossy()),
                &target,
                None,
                MsFlags::MS_BIND,
                None,
            );
        }
    }

    let sandbox_bin = rootfs.join("usr/bin");
    let _ = std::fs::create_dir_all(&sandbox_bin);
    for bin in &[
        "nvidia-smi",
        "nvidia-debugdump",
        "nvidia-cuda-mps-control",
        "nvidia-persistenced",
    ] {
        let host = Path::new("/usr/bin").join(bin);
        if !host.exists() {
            continue;
        }
        let target = sandbox_bin.join(bin);
        if std::fs::File::create(&target).is_ok() {
            let _ = do_mount(
                bin,
                Some(&host.to_string_lossy()),
                &target,
                None,
                MsFlags::MS_BIND,
                None,
            );
        }
    }
}
