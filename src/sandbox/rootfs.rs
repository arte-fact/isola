use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::error::{IoContext, IsolaError};
use crate::paths;
use crate::plugin::PluginRegistry;
use crate::sandbox::config::SandboxShell;

const ROOTFS_URL: &str = "https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/ubuntu-base-24.04.4-base-amd64.tar.gz";
const ROOTFS_FILENAME: &str = "ubuntu-base-24.04.4-base-amd64.tar.gz";
const ROOTFS_SHA256SUMS_URL: &str =
    "https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/SHA256SUMS";

// --- Provisioning script fragments ---

const PROVISION_BASE: &str = r#"
set -eo pipefail
export DEBIAN_FRONTEND=noninteractive

# Fix file ownership for user namespace (files extracted as host UID appear
# as non-root inside multi-UID namespaces, which can break dpkg/apt)
for d in /var/lib/dpkg /var/cache/apt /var/log /etc/apt /etc/dpkg /run; do
    [ -d "$d" ] && chown -R 0:0 "$d" 2>/dev/null || true
done

echo ">>> Updating package lists..."
apt-get update -y

echo ">>> Installing essential packages..."
apt-get install -y --no-install-recommends \
    apt-utils ca-certificates curl wget git \
    build-essential pkg-config libssl-dev \
    sudo

echo ">>> Installing additional packages..."
apt-get install -y --no-install-recommends \
    ncurses-base \
    jq tree htop tmux \
    zip unzip tar gzip \
    ripgrep fd-find bat \
    less file lsof strace \
    openssh-client gnupg \
    net-tools dnsutils iproute2 \
    man-db \
    || true
dpkg --configure -a --force-overwrite 2>/dev/null || true

# Create convenience symlinks for tools with non-obvious binary names
ln -sf /usr/bin/fdfind /usr/local/bin/fd 2>/dev/null || true
ln -sf /usr/bin/batcat /usr/local/bin/bat 2>/dev/null || true
"#;

/// Read the host's git user.name and user.email, returning a .gitconfig string if either is set.
fn build_host_gitconfig() -> Option<String> {
    let name = std::process::Command::new("git")
        .args(["config", "--global", "user.name"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    let email = std::process::Command::new("git")
        .args(["config", "--global", "user.email"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    if name.is_none() && email.is_none() {
        return None;
    }

    let mut cfg = String::from("[user]\n");
    if let Some(n) = name {
        cfg.push_str(&format!("\tname = {n}\n"));
    }
    if let Some(e) = email {
        cfg.push_str(&format!("\temail = {e}\n"));
    }
    Some(cfg)
}

/// Build a provisioning script from selected environments using the plugin registry.
pub fn build_provision_script(
    environments: &[String],
    shell: &SandboxShell,
    registry: &PluginRegistry,
) -> String {
    let mut script = String::from("#!/bin/bash\n");
    script.push_str("export PATH=\"/root/.cargo/bin:/root/.local/bin:/usr/local/go/bin:$PATH\"\n");

    // Base packages (always)
    script.push_str(PROVISION_BASE);

    // Create sandbox user before plugins (some plugins like gpu need usermod)
    script.push_str(&format!(
        r#"
echo ">>> Creating sandbox user..."
groupadd -g 1000 sandbox 2>/dev/null || true
useradd -m -u 1000 -g 1000 -s {} sandbox 2>/dev/null || true"#,
        shell.bin_path()
    ));
    script.push_str(
        r#"

# Set password (chpasswd needs PAM which may not work in user namespaces;
# fall back to writing the shadow entry directly)
if ! echo "sandbox:sandbox" | chpasswd 2>/dev/null; then
    # Pre-computed SHA-512 hash for "sandbox" with salt "isola"
    HASH='$6$isola$TE6K9kO1QWE0643fNqowg9NUHwMOHuZBO0UinG8mvQyjs.IYapfKU.jf8LMshE726lAsRVjWyyVw8X6r7rBr1.'
    sed -i "s|^sandbox:[^:]*:|sandbox:${HASH}:|" /etc/shadow 2>/dev/null || true
fi

mkdir -p /etc/sudoers.d
echo "sandbox ALL=(ALL:ALL) NOPASSWD:ALL" > /etc/sudoers.d/sandbox
chmod 440 /etc/sudoers.d/sandbox

mkdir -p /run/user/0 /run/user/1000
chown 0:0 /run/user/0
chown 1000:1000 /run/user/1000
chmod 700 /run/user/0 /run/user/1000
"#,
    );

    // Selected environments (from plugins; shell plugins run here too; config-only plugins skipped)
    for env in environments {
        if let Some(plugin) = registry.get(env) {
            if let Some(ref install_script) = plugin.install_script {
                script.push('\n');
                script.push_str(install_script);
            }
        } else {
            eprintln!("warning: unknown environment '{env}', skipping");
        }
    }

    // Copy tools to sandbox user (from plugin paths.copy)
    let mut path_parts = Vec::new();
    for env in environments {
        if let Some(plugin) = registry.get(env) {
            for cp in &plugin.manifest.paths.copy {
                if let Some(parent) = std::path::Path::new(&cp.to).parent() {
                    script.push_str(&format!("mkdir -p {}\n", parent.display()));
                }
                script.push_str(&format!(
                    "cp -rp {} {} 2>/dev/null || true\n",
                    cp.from, cp.to
                ));
            }
            for bin in &plugin.manifest.paths.bin {
                path_parts.push(bin.as_str());
            }
        }
    }

    script.push_str("chown -R 1000:1000 /home/sandbox/\n");

    // Ensure all files directly in plugin bin directories are executable.
    // Some installers (e.g. Claude Code) target the sandbox user directly and
    // create files without the execute bit; this fixes it generically.
    for env in environments {
        if let Some(plugin) = registry.get(env) {
            for bin in &plugin.manifest.paths.bin {
                script.push_str(&format!(
                    "find {bin} -maxdepth 1 -type f -exec chmod a+x {{}} + 2>/dev/null || true\n"
                ));
            }
        }
    }

    // Build PATH for sandbox user
    path_parts.push("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
    let path_line = format!(
        "echo 'export PATH=\"{}\"' >> /home/sandbox/.bashrc\n",
        path_parts.join(":")
    );
    script.push_str(&path_line);

    // Root PATH (collect all root-side bin paths from plugins)
    let mut root_path_parts: Vec<String> = Vec::new();
    for env in environments {
        if let Some(plugin) = registry.get(env) {
            for bin in &plugin.manifest.paths.bin {
                if let Some(rest) = bin.strip_prefix("/home/sandbox/") {
                    root_path_parts.push(format!("/root/{rest}"));
                } else {
                    root_path_parts.push(bin.clone());
                }
            }
        }
    }
    root_path_parts.push("$PATH".to_string());
    script.push_str(&format!(
        "echo 'export PATH=\"{}\"' >> /root/.bashrc\n",
        root_path_parts.join(":")
    ));

    // Verify
    script.push_str("\necho \">>> Verifying...\"\n");
    for env in environments {
        if let Some(plugin) = registry.get(env)
            && let Some(verify) = &plugin.manifest.provision.verify
        {
            script.push_str(verify);
            script.push('\n');
        }
    }
    script.push_str("id sandbox\n");
    script.push_str("echo \"=== Provisioning complete ===\"\n");

    script
}

/// Download and cache rootfs with progress UI.
pub fn ensure_rootfs_cached_with_progress(
    progress: &crate::progress::CreationProgress,
) -> Result<PathBuf, IsolaError> {
    let cache = paths::cache_dir();
    let cached_path = cache.join(ROOTFS_FILENAME);

    if cached_path.exists() {
        progress.finish_step("Rootfs cached");
        return Ok(cached_path);
    }

    std::fs::create_dir_all(&cache).io_ctx("create cache dir", &cache)?;

    let response = reqwest::blocking::get(ROOTFS_URL)?;
    if !response.status().is_success() {
        return Err(IsolaError::ExtractionFailed(format!(
            "HTTP {}",
            response.status()
        )));
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut dl = progress.start_download(total_size);

    let mut reader = response;
    let mut file = std::fs::File::create(&cached_path).io_ctx("create", &cached_path)?;
    let mut buf = [0u8; 8192];
    let mut downloaded: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).io_ctx("write to", &cached_path)?;
        downloaded += n as u64;
        dl.set_position(downloaded);
    }
    progress.finish_download();

    progress.start_step("Verifying checksum...");
    verify_rootfs_checksum(&cached_path)?;
    progress.finish_step("Checksum verified");

    Ok(cached_path)
}

/// Verify the rootfs tarball against Ubuntu's published SHA256SUMS.
fn verify_rootfs_checksum(path: &Path) -> Result<(), IsolaError> {
    use std::io::Read;

    // Fetch SHA256SUMS from Ubuntu
    let response = reqwest::blocking::get(ROOTFS_SHA256SUMS_URL);
    let sums_text = match response {
        Ok(r) if r.status().is_success() => match r.text() {
            Ok(t) => t,
            Err(_) => {
                eprintln!(
                    "warning: could not read SHA256SUMS response body, skipping verification"
                );
                return Ok(());
            }
        },
        _ => {
            eprintln!("warning: could not fetch SHA256SUMS for verification, skipping");
            return Ok(());
        }
    };

    // Find expected hash for our filename
    let expected_hash = sums_text
        .lines()
        .find(|line| line.contains(ROOTFS_FILENAME))
        .and_then(|line| line.split_whitespace().next());

    let expected_hash = match expected_hash {
        Some(h) => h.to_string(),
        None => {
            eprintln!("warning: rootfs filename not found in SHA256SUMS, skipping verification");
            return Ok(());
        }
    };

    // Compute SHA256 of downloaded file
    let mut file = std::fs::File::open(path).io_ctx("open", path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).io_ctx("read", path)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual_hash = hasher.finish_hex();

    if actual_hash != expected_hash {
        // Remove the corrupted file so the next attempt re-downloads
        let _ = std::fs::remove_file(path);
        return Err(IsolaError::ExtractionFailed(format!(
            "SHA256 mismatch: expected {expected_hash}, got {actual_hash}"
        )));
    }

    Ok(())
}

/// Minimal SHA256 implementation (no external crate needed).
struct Sha256 {
    state: [u32; 8],
    buffer: Vec<u8>,
    total_len: u64,
}

impl Sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: Vec::new(),
            total_len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u64;
        self.buffer.extend_from_slice(data);
        while self.buffer.len() >= 64 {
            let block: [u8; 64] = self.buffer[..64].try_into().unwrap();
            self.process_block(&block);
            self.buffer.drain(..64);
        }
    }

    fn process_block(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..(i + 1) * 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;

        #[allow(clippy::needless_range_loop)]
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(Self::K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }

    fn finish_hex(mut self) -> String {
        let bit_len = self.total_len * 8;
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0);
        }
        self.buffer.extend_from_slice(&bit_len.to_be_bytes());
        while self.buffer.len() >= 64 {
            let block: [u8; 64] = self.buffer[..64].try_into().unwrap();
            self.process_block(&block);
            self.buffer.drain(..64);
        }
        self.state.iter().map(|v| format!("{v:08x}")).collect()
    }
}

pub fn extract_rootfs(tarball: &Path, target: &Path) -> Result<(), IsolaError> {
    std::fs::create_dir_all(target).io_ctx("create rootfs target dir", target)?;

    let file = std::fs::File::open(tarball).io_ctx("open rootfs tarball", tarball)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(true);
    archive.set_preserve_ownerships(false);
    archive.set_unpack_xattrs(false);

    archive.unpack(target).map_err(|e| {
        IsolaError::ExtractionFailed(format!("{} (tarball: {})", e, tarball.display()))
    })?;

    ensure_usr_merge_links(target)?;
    Ok(())
}

/// Re-assert ubuntu-base's usr-merge symlinks (`/bin /lib /lib64 /sbin -> usr/*`) after an
/// extraction. Layered cache tarballs can lose top-level symlink entries (tar refuses to
/// write through a symlink path component, and a layer carrying real `lib/...` paths ends
/// up clobbering the link), which leaves the dynamic loader unresolvable — every execve in
/// the sandbox then fails ENOENT. The links are an invariant of the rootfs, so restore any
/// that are missing whenever the merged `usr/<dir>` exists.
fn ensure_usr_merge_links(rootfs: &Path) -> Result<(), IsolaError> {
    for dir in ["bin", "lib", "lib64", "sbin"] {
        let link = rootfs.join(dir);
        if std::fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        if rootfs.join("usr").join(dir).is_dir() {
            std::os::unix::fs::symlink(format!("usr/{dir}"), &link)
                .io_ctx("restore usr-merge symlink", &link)?;
        }
    }
    Ok(())
}

/// Verify that a rootfs contains a working `/bin/bash`. Provisioning and
/// `run_command_captured` exec `/bin/bash` directly; a missing or unresolvable
/// path surfaces as a cryptic `execve failed: ENOENT` inside the child. Call
/// this after every `extract_rootfs` to fail fast with a clear message.
pub fn ensure_rootfs_has_bash(rootfs: &Path) -> Result<(), IsolaError> {
    let bash = rootfs.join("bin/bash");
    if bash.exists() {
        return Ok(());
    }
    Err(IsolaError::ExtractionFailed(format!(
        "rootfs at {} has no working /bin/bash after extraction — the cached \
         layer tarball is likely truncated (e.g. a prior caching step failed \
         partway). Try: rm -rf ~/.isola/cache/layers && re-run with --no-cache",
        rootfs.display(),
    )))
}

/// A provisioned-cache extract must yield a *complete* rootfs — a torn or mangled tarball
/// (the retired layered cache produced both) surfaces as cryptic in-sandbox failures long
/// after create. Cheap invariants: a resolvable shell and the dpkg database.
pub fn sanity_check_provisioned(rootfs: &Path) -> Result<(), IsolaError> {
    for rel in ["bin/bash", "var/lib/dpkg/status"] {
        if !rootfs.join(rel).exists() {
            return Err(IsolaError::ExtractionFailed(format!(
                "cached rootfs at {} is incomplete (missing /{rel}) — discarding this cache",
                rootfs.display(),
            )));
        }
    }
    Ok(())
}

/// Recursively copy `src` into `dst`, preserving symlinks verbatim.
///
/// User config dirs (e.g. fish, omf, nvim) routinely contain symlinks — some
/// of them dangling on the host. `fs::copy` follows symlinks and ENOENT's on
/// broken ones; `Path::is_dir` also follows and misclassifies them. Inspect
/// `symlink_metadata` instead and recreate symlinks as-is so broken links
/// come along silently (they resolved on the host, or didn't, same deal
/// inside the sandbox).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), IsolaError> {
    std::fs::create_dir_all(dst).io_ctx("create dir", dst)?;
    let read = std::fs::read_dir(src).io_ctx("read dir", src)?;
    for entry in read {
        let entry = entry.io_ctx("read dir entry", src)?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let meta = std::fs::symlink_metadata(&src_path).io_ctx("stat", &src_path)?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            let target = std::fs::read_link(&src_path).io_ctx("readlink", &src_path)?;
            // Ensure we can write the link even if something already exists there.
            let _ = std::fs::remove_file(&dst_path);
            std::os::unix::fs::symlink(&target, &dst_path).io_ctx("symlink", &dst_path)?;
        } else if ft.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).io_ctx("copy", &src_path)?;
        }
    }
    Ok(())
}

pub fn post_setup_rootfs(
    rootfs: &Path,
    name: &str,
    environments: &[String],
    registry: &PluginRegistry,
) -> Result<(), IsolaError> {
    // Inherit DNS from host. `/etc/resolv.conf` is often a symlink into
    // `/run/systemd/resolve/` — if the target is gone (e.g. resolved is
    // disabled) read_to_string returns ENOENT. Fall back to a sane default
    // so sandbox creation doesn't fail on a host-side quirk.
    let host_resolv = match std::fs::read_to_string("/etc/resolv.conf") {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "warning: /etc/resolv.conf unreadable ({e}); using public DNS fallback in sandbox"
            );
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n".to_string()
        }
        Err(e) => {
            return Err(IsolaError::IoAt {
                op: "read",
                path: PathBuf::from("/etc/resolv.conf"),
                source: e,
            });
        }
    };
    let resolv_dst = rootfs.join("etc/resolv.conf");
    std::fs::write(&resolv_dst, &host_resolv).io_ctx("write", &resolv_dst)?;

    let hostname_dst = rootfs.join("etc/hostname");
    std::fs::write(&hostname_dst, format!("{name}\n")).io_ctx("write", &hostname_dst)?;

    let hosts_dst = rootfs.join("etc/hosts");
    std::fs::write(
        &hosts_dst,
        format!("127.0.0.1 localhost {name}\n::1 localhost {name}\n"),
    )
    .io_ctx("write", &hosts_dst)?;

    let apt_dir = rootfs.join("etc/apt/apt.conf.d");
    std::fs::create_dir_all(&apt_dir).io_ctx("create dir", &apt_dir)?;
    let apt_cfg = apt_dir.join("99sandbox");
    std::fs::write(&apt_cfg, "APT::Sandbox::User \"root\";\n").io_ctx("write", &apt_cfg)?;

    // Configure dpkg for user namespace environment
    let dpkg_dir = rootfs.join("etc/dpkg/dpkg.cfg.d");
    std::fs::create_dir_all(&dpkg_dir).io_ctx("create dir", &dpkg_dir)?;
    let dpkg_cfg = dpkg_dir.join("01sandbox");
    std::fs::write(&dpkg_cfg, "force-unsafe-io\n").io_ctx("write", &dpkg_cfg)?;

    for dir in &[
        "workspace",
        "root",
        "dev",
        "proc",
        "sys",
        "tmp",
        "usr/local/bin",
    ] {
        let d = rootfs.join(dir);
        std::fs::create_dir_all(&d).io_ctx("create dir", &d)?;
    }

    // Copy host files specified by plugins (host_copy)
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    if let Some(ref h) = home {
        for env in environments {
            if let Some(plugin) = registry.get(env) {
                for entry in &plugin.manifest.paths.host_copy {
                    let src = h.join(&entry.from);
                    let dst = rootfs.join("home/sandbox").join(&entry.to);
                    if src.exists() {
                        if src.is_dir() {
                            copy_dir_recursive(&src, &dst)?;
                        } else {
                            if let Some(parent) = dst.parent() {
                                std::fs::create_dir_all(parent).io_ctx("create dir", parent)?;
                            }
                            std::fs::copy(&src, &dst).io_ctx("copy", &src)?;
                        }
                    }
                }
            }
        }
    }

    // Inherit host git identity (best-effort). The user dirs may not exist yet on the
    // fresh-extract path (provisioning creates them later); write must not depend on order.
    if let Some(gitconfig) = build_host_gitconfig() {
        for rel in ["home/sandbox/.gitconfig", "root/.gitconfig"] {
            let path = rootfs.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).io_ctx("create dir", parent)?;
            }
            std::fs::write(&path, &gitconfig).io_ctx("write", &path)?;
        }
    }

    Ok(())
}

pub fn rootfs_url() -> &'static str {
    ROOTFS_URL
}

/// Check if a cached provisioned rootfs exists for the given environments.
pub fn has_cached_provision(environments: &[String], shell: &str) -> Option<PathBuf> {
    let path = paths::provision_cache_path(environments, shell);
    if path.exists() { Some(path) } else { None }
}

/// In-sandbox path where the cache scripts deposit their tarballs before we
/// move them out to the host cache directory.
const PROVISION_CACHE_FILE: &str = ".provision_cache.tar.gz";

/// Build a script that tars the entire rootfs into `/var/tmp/{out}` from inside
/// the sandbox — NOT `/tmp`, which the namespace overmounts with a tmpfs that
/// evaporates on exit (the tarball must land in the real rootfs for the host
/// to move it into the cache).
///
/// Provisioning leaves files owned by the in-namespace sandbox user (UID
/// 1000), which the host sees as an unmapped subuid. A host-side `tar` over
/// `~/.isola/sandboxes/<name>/rootfs` therefore fails with EACCES on those
/// files. Running tar inside the sandbox sidesteps that — every file is
/// readable by the in-namespace root.
fn build_full_rootfs_cache_script(out: &str) -> String {
    let stem = out.trim_end_matches(".tar.gz");
    format!(
        r#"#!/bin/bash
set -eo pipefail
echo ">>> Capturing rootfs cache..."
rm -f /var/tmp/{out} /var/tmp/{stem}.partial
cd /
tar czf /var/tmp/{stem}.partial \
    --exclude=./proc/* \
    --exclude=./sys/* \
    --exclude=./dev/* \
    --exclude=./tmp/* \
    --exclude=./run/* \
    --exclude=./var/tmp/* \
    --exclude=./workspace/* \
    .
mv /var/tmp/{stem}.partial /var/tmp/{out}
[ -s /var/tmp/{out} ] || {{ echo "error: cache tarball is empty" >&2; exit 1; }}
echo "=== Rootfs cache complete ==="
"#
    )
}

/// Script that creates the full-provision cache tarball at
/// `/var/tmp/.provision_cache.tar.gz` inside the sandbox.
pub fn build_provision_cache_script() -> String {
    build_full_rootfs_cache_script(PROVISION_CACHE_FILE)
}

/// Move a cache tarball produced inside the sandbox to its final host path.
fn move_cache_tarball(src: &Path, dest: &Path) -> Result<(), IsolaError> {
    if !src.exists() {
        return Err(IsolaError::ExtractionFailed(format!(
            "expected cache tarball at {} but the in-sandbox cache script did not produce it",
            src.display(),
        )));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).io_ctx("create cache dir", parent)?;
    }
    std::fs::rename(src, dest).io_ctx("move cache tarball into host cache", src)?;
    Ok(())
}

/// Move the full-provision tarball (created by `build_provision_cache_script`)
/// out of the sandbox into the host cache directory.
pub fn cache_provisioned_rootfs(
    name: &str,
    environments: &[String],
    shell: &str,
) -> Result<(), IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    let src = rootfs_path.join("var/tmp").join(PROVISION_CACHE_FILE);
    let dest = paths::provision_cache_path(environments, shell);
    move_cache_tarball(&src, &dest)
}

/// Minimal script to fix file ownership after extracting a cached provisioned rootfs.
/// After extraction with set_preserve_ownerships(false), all files are root-owned inside
/// the namespace. This restores sandbox user ownership on /home/sandbox/.
pub fn build_fixup_script() -> String {
    "#!/bin/bash\nset -e\nchown -R 1000:1000 /home/sandbox/\n".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginRegistry;

    fn registry() -> PluginRegistry {
        PluginRegistry::load().unwrap()
    }

    #[test]
    fn provision_script_base_only() {
        let r = registry();
        let script = build_provision_script(&[], &SandboxShell::Bash, &r);
        assert!(script.contains("apt-get update"));
        assert!(script.contains("Creating sandbox user"));
        assert!(!script.contains("Installing Rust"));
        assert!(!script.contains("Installing Node"));
    }

    #[test]
    fn provision_script_includes_selected_envs() {
        let r = registry();
        let envs = vec!["rust".to_string(), "nodejs".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Bash, &r);
        assert!(script.contains("Installing Rust"));
        assert!(script.contains("Installing Node.js"));
        assert!(!script.contains("Installing Python"));
        assert!(!script.contains("Installing Go"));
    }

    #[test]
    fn provision_script_all_envs() {
        let r = registry();
        let envs = vec![
            "rust".to_string(),
            "nodejs".to_string(),
            "python-uv".to_string(),
            "go".to_string(),
        ];
        let script = build_provision_script(&envs, &SandboxShell::Bash, &r);
        assert!(script.contains("Installing Rust"));
        assert!(script.contains("Installing Node.js"));
        assert!(script.contains("Installing Python"));
        assert!(script.contains("Installing Go"));
        assert!(script.contains("rustc --version"));
        assert!(script.contains("node --version"));
        assert!(script.contains("python3 --version"));
        assert!(script.contains("go version"));
    }

    #[test]
    fn provision_script_copies_tools_for_rust() {
        let r = registry();
        let envs = vec!["rust".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Bash, &r);
        assert!(script.contains("/home/sandbox/.cargo/bin"));
        assert!(script.contains("cp -rp /root/.cargo /home/sandbox/.cargo"));
    }

    #[test]
    fn provision_script_unknown_env_ignored() {
        let r = registry();
        let envs = vec!["unknown-env".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Bash, &r);
        assert!(script.contains("apt-get update"));
    }

    #[test]
    fn provision_script_installs_fish() {
        let r = registry();
        let envs = vec!["fish".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Fish, &r);
        assert!(script.contains("Installing fish shell"));
        assert!(script.contains("/usr/bin/fish"));
    }

    #[test]
    fn provision_script_installs_zsh() {
        let r = registry();
        let envs = vec!["zsh".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Zsh, &r);
        assert!(script.contains("Installing zsh"));
        assert!(script.contains("/usr/bin/zsh"));
    }

    #[test]
    fn provision_script_installs_neovim() {
        let r = registry();
        let envs = vec!["neovim".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Bash, &r);
        assert!(script.contains("Installing neovim"));
    }

    #[test]
    fn provision_script_creates_user_before_plugins() {
        let r = registry();
        let envs = vec!["gpu".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Bash, &r);
        let user_pos = script.find("Creating sandbox user").unwrap();
        let gpu_pos = script.find("Setting up GPU access").unwrap();
        assert!(
            user_pos < gpu_pos,
            "sandbox user must be created before GPU plugin runs"
        );
    }

    #[test]
    fn extract_rootfs_preserves_usrmerge_bin_symlink() {
        // Ubuntu 24.04 rootfs has `bin -> usr/bin` (usrmerge). The tarball
        // declares `bin` as a symlink and `usr/bin/bash` as a file. Extraction
        // must create both so `<root>/bin/bash` resolves.
        use flate2::write::GzEncoder;

        let tmp = tempfile::tempdir().unwrap();
        let tar_path = tmp.path().join("tiny-rootfs.tar.gz");

        // Build a minimal tarball that mimics usrmerge ordering.
        {
            let file = std::fs::File::create(&tar_path).unwrap();
            let encoder = GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);

            // ./bin -> usr/bin (symlink, appearing before usr)
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_path("bin").unwrap();
            header.set_link_name("usr/bin").unwrap();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();

            // ./usr/bin/bash (file)
            let bash_bytes = b"#!/placeholder\n";
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_path("usr/bin/bash").unwrap();
            header.set_size(bash_bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append(&header, std::io::Cursor::new(bash_bytes))
                .unwrap();

            builder.into_inner().unwrap().finish().unwrap();
        }

        let out = tmp.path().join("out");
        extract_rootfs(&tar_path, &out).unwrap();

        let bin = out.join("bin");
        assert!(
            bin.symlink_metadata().unwrap().file_type().is_symlink(),
            "expected {} to be a symlink",
            bin.display()
        );
        assert!(
            out.join("usr/bin/bash").exists(),
            "expected usr/bin/bash to exist"
        );
        assert!(
            bin.join("bash").exists(),
            "expected bin/bash to resolve via symlink"
        );

        // Round-trip through tar::Builder::append_dir_all to confirm the tar
        // crate preserves the usrmerge symlink on write as well as read.
        let recached = tmp.path().join("recached.tar.gz");
        let file = std::fs::File::create(&recached).unwrap();
        let encoder = GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        builder.follow_symlinks(false);
        builder.append_dir_all(".", &out).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let out2 = tmp.path().join("out2");
        extract_rootfs(&recached, &out2).unwrap();
        assert!(
            out2.join("bin")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink(),
            "after round-trip, bin must still be a symlink"
        );
        assert!(
            out2.join("bin/bash").exists(),
            "after round-trip, bin/bash must resolve"
        );
    }

    #[test]
    fn ensure_rootfs_has_bash_ok_when_bash_resolves_via_symlink() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::write(root.join("usr/bin/bash"), b"#!/x\n").unwrap();
        symlink("usr/bin", root.join("bin")).unwrap();
        ensure_rootfs_has_bash(root).unwrap();
    }

    #[test]
    fn ensure_rootfs_has_bash_errors_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let err = ensure_rootfs_has_bash(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no working /bin/bash"), "got: {msg}");
    }

    #[test]
    fn copy_dir_recursive_preserves_broken_symlinks() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join("functions")).unwrap();
        // Regular file: should copy as a regular file.
        std::fs::write(src.join("functions/real.fish"), b"echo hi\n").unwrap();
        // Broken symlink: target does not exist — real-world case is
        // ~/.config/fish/functions/fzf_key_bindings.fish pointing at a
        // package-manager-installed file that's since been removed.
        symlink(
            "/nonexistent/fzf_key_bindings.fish",
            src.join("functions/broken.fish"),
        )
        .unwrap();
        // Valid symlink to a sibling file.
        symlink("real.fish", src.join("functions/alias.fish")).unwrap();

        copy_dir_recursive(&src, &dst).expect("broken symlinks must not fail the copy");

        let broken = dst.join("functions/broken.fish");
        let broken_meta = std::fs::symlink_metadata(&broken).unwrap();
        assert!(
            broken_meta.file_type().is_symlink(),
            "broken link must be preserved as a symlink, not dereferenced"
        );
        assert!(!broken.exists(), "broken link must remain dangling");

        let alias = dst.join("functions/alias.fish");
        assert!(
            std::fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink(),
        );
        assert!(alias.exists(), "valid symlink must resolve");

        let real = dst.join("functions/real.fish");
        assert!(real.is_file());
    }

    #[test]
    fn provision_cache_script_uses_partial_then_rename() {
        let s = build_provision_cache_script();
        // Atomic: write to .partial, then mv into place — never leave a
        // truncated .tar.gz that future runs would treat as a valid cache.
        assert!(s.contains(".provision_cache.partial"));
        assert!(
            s.contains("mv /var/tmp/.provision_cache.partial /var/tmp/.provision_cache.tar.gz")
        );
        // /tmp is namespace-tmpfs (evaporates); the tarball must live in the real rootfs.
        assert!(!s.contains("czf /tmp/"));
        // Fail-fast on any error so a partial tarball doesn't get renamed.
        assert!(s.contains("set -eo pipefail"));
    }

    #[test]
    fn cache_script_excludes_virtual_filesystems_and_workspace() {
        // /proc /sys /dev /run are kernel virtual fs, /tmp would recurse into
        // the tarball itself, /workspace is a host bind-mount that must never
        // end up in the cached image.
        for s in [build_provision_cache_script()] {
            for excl in [
                "./proc/*",
                "./sys/*",
                "./dev/*",
                "./tmp/*",
                "./run/*",
                "./var/tmp/*",
                "./workspace/*",
            ] {
                assert!(s.contains(excl), "missing exclusion {excl} in:\n{s}");
            }
        }
    }

    #[test]
    fn move_cache_tarball_errors_with_clear_message_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("not_there.tar.gz");
        let dest = tmp.path().join("dest/cache.tar.gz");

        let err = move_cache_tarball(&src, &dest).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("in-sandbox cache script"),
            "error should explain the missing tarball means the in-sandbox script didn't run; got: {msg}"
        );
        assert!(!dest.exists());
    }

    #[test]
    fn move_cache_tarball_renames_into_place_creating_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src.tar.gz");
        std::fs::write(&src, b"fake-tarball").unwrap();
        let dest = tmp.path().join("nested/dir/cache.tar.gz");

        move_cache_tarball(&src, &dest).unwrap();

        assert!(!src.exists(), "src should have been moved");
        assert_eq!(std::fs::read(&dest).unwrap(), b"fake-tarball");
    }

    // SHA256 test vectors from NIST / RFC 6234
    #[test]
    fn sha256_empty_string() {
        let mut h = Sha256::new();
        h.update(b"");
        assert_eq!(
            h.finish_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_abc() {
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(
            h.finish_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_long_message() {
        let mut h = Sha256::new();
        h.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            h.finish_hex(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_incremental_update() {
        let mut h = Sha256::new();
        h.update(b"abc");
        h.update(b"dbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            h.finish_hex(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}

#[cfg(test)]
mod usr_merge_tests {
    use super::*;

    #[test]
    fn extraction_restores_lost_usr_merge_links() {
        // A layer tarball that carries real `lib/...` paths but no top-level symlink
        // entries (the observed cache corruption): after extraction the usr-merge links
        // must exist again so the dynamic loader resolves.
        let dir = std::env::temp_dir().join(format!("isola-usrmerge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("usr/lib64")).unwrap();
        std::fs::create_dir_all(src.join("usr/bin")).unwrap();
        std::fs::write(src.join("usr/lib64/ld-linux-x86-64.so.2"), "x").unwrap();
        let tar_path = dir.join("layer.tar.gz");
        let out = std::process::Command::new("tar")
            .args(["czf"])
            .arg(&tar_path)
            .args(["-C"])
            .arg(&src)
            .arg(".")
            .output()
            .unwrap();
        assert!(out.status.success());
        let target = dir.join("rootfs");
        extract_rootfs(&tar_path, &target).unwrap();
        for link in ["bin", "lib64"] {
            let meta = std::fs::symlink_metadata(target.join(link)).unwrap();
            assert!(meta.file_type().is_symlink(), "{link} must be a symlink");
        }
        // No usr/sbin in this rootfs → no sbin link fabricated.
        assert!(std::fs::symlink_metadata(target.join("sbin")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
