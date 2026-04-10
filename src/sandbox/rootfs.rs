use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::error::IsolaError;
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

    std::fs::create_dir_all(&cache)?;

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
    let mut file = std::fs::File::create(&cached_path)?;
    let mut buf = [0u8; 8192];
    let mut downloaded: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
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
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
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
    std::fs::create_dir_all(target)?;

    let file = std::fs::File::open(tarball)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(true);
    archive.set_preserve_ownerships(false);
    archive.set_unpack_xattrs(false);

    archive
        .unpack(target)
        .map_err(|e| IsolaError::ExtractionFailed(e.to_string()))?;

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), IsolaError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
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
    let host_resolv = std::fs::read_to_string("/etc/resolv.conf")?;
    std::fs::write(rootfs.join("etc/resolv.conf"), &host_resolv)?;

    std::fs::write(rootfs.join("etc/hostname"), format!("{name}\n"))?;

    std::fs::write(
        rootfs.join("etc/hosts"),
        format!("127.0.0.1 localhost {name}\n::1 localhost {name}\n"),
    )?;

    std::fs::create_dir_all(rootfs.join("etc/apt/apt.conf.d"))?;
    std::fs::write(
        rootfs.join("etc/apt/apt.conf.d/99sandbox"),
        "APT::Sandbox::User \"root\";\n",
    )?;

    // Configure dpkg for user namespace environment
    std::fs::create_dir_all(rootfs.join("etc/dpkg/dpkg.cfg.d"))?;
    std::fs::write(
        rootfs.join("etc/dpkg/dpkg.cfg.d/01sandbox"),
        "force-unsafe-io\n",
    )?;

    for dir in &[
        "workspace",
        "root",
        "dev",
        "proc",
        "sys",
        "tmp",
        "usr/local/bin",
    ] {
        std::fs::create_dir_all(rootfs.join(dir))?;
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
                                std::fs::create_dir_all(parent)?;
                            }
                            std::fs::copy(&src, &dst)?;
                        }
                    }
                }
            }
        }
    }

    // Inherit host git identity (best-effort)
    if let Some(gitconfig) = build_host_gitconfig() {
        std::fs::write(rootfs.join("home/sandbox/.gitconfig"), &gitconfig)?;
        std::fs::write(rootfs.join("root/.gitconfig"), &gitconfig)?;
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

/// Create a gzipped tarball of the provisioned rootfs for future reuse.
pub fn cache_provisioned_rootfs(
    name: &str,
    environments: &[String],
    shell: &str,
) -> Result<(), IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    let cache_path = paths::provision_cache_path(environments, shell);

    let file = std::fs::File::create(&cache_path)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    builder
        .append_dir_all(".", &rootfs_path)
        .map_err(|e| IsolaError::ExtractionFailed(format!("cache tarball: {e}")))?;
    builder
        .into_inner()
        .map_err(|e| IsolaError::ExtractionFailed(format!("cache finalize: {e}")))?
        .finish()
        .map_err(|e| IsolaError::ExtractionFailed(format!("cache compress: {e}")))?;

    Ok(())
}

/// Minimal script to fix file ownership after extracting a cached provisioned rootfs.
/// After extraction with set_preserve_ownerships(false), all files are root-owned inside
/// the namespace. This restores sandbox user ownership on /home/sandbox/.
pub fn build_fixup_script() -> String {
    "#!/bin/bash\nset -e\nchown -R 1000:1000 /home/sandbox/\n".to_string()
}

// --- Layered cache support ---

/// Compute a short hash of a provisioning script fragment for cache keying.
/// Returns the first 12 hex characters of SHA256(script_text).
pub fn layer_version_hash(script_text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(script_text.as_bytes());
    hasher.finish_hex()[..12].to_string()
}

/// Script fragment for capturing layer diff via marker file.
const LAYER_CAPTURE_HEADER: &str = r#"#!/bin/bash
set -eo pipefail
export DEBIAN_FRONTEND=noninteractive
export PATH="/root/.cargo/bin:/root/.local/bin:/usr/local/go/bin:$PATH"
touch /tmp/.layer_marker
sleep 1
"#;

const LAYER_CAPTURE_FOOTER: &str = r#"
echo ">>> Capturing layer..."
find / -xdev -newer /tmp/.layer_marker \
    ! -path '/proc/*' ! -path '/sys/*' ! -path '/dev/*' ! -path '/tmp/*' \
    ! -path '/workspace/*' ! -path '/run/*' \
    -print0 2>/dev/null | \
    tar czf /tmp/.layer_cache.tar.gz --null -T - 2>/dev/null || true
echo "=== Layer complete ==="
"#;

/// Build a provisioning script for the base layer (packages + shell + user creation).
pub fn build_base_layer_script(shell: &SandboxShell) -> String {
    let mut script = String::from("#!/bin/bash\n");
    script.push_str("export PATH=\"/root/.cargo/bin:/root/.local/bin:/usr/local/go/bin:$PATH\"\n");
    script.push_str("export DEBIAN_FRONTEND=noninteractive\n");

    script.push_str(PROVISION_BASE);

    // Create sandbox user
    script.push_str(&format!(
        r#"
echo ">>> Creating sandbox user..."
groupadd -g 1000 sandbox 2>/dev/null || true
useradd -m -u 1000 -g 1000 -s {} sandbox 2>/dev/null || true"#,
        shell.bin_path()
    ));
    script.push_str(
        r#"
if ! echo "sandbox:sandbox" | chpasswd 2>/dev/null; then
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

    // isola sandbox prompt marker for bash
    script.push_str(
        r#"cat >> /home/sandbox/.bashrc << 'ISOLA_BASH_EOF'
if [ -n "${ISOLA_SANDBOX:-}" ]; then
    PS1="\[\033[0;36m\](isola:${ISOLA_SANDBOX})\[\033[0m\] ${PS1}"
fi
ISOLA_BASH_EOF
"#,
    );

    script.push_str("echo \"=== Base layer complete ===\"\n");
    script
}

/// Build a provisioning script for a single environment layer using its plugin.
/// The script uses a marker file to capture only the files changed by this layer.
/// Returns None if the plugin is not found or has no install script (config-only plugin).
pub fn build_env_layer_script(env_name: &str, registry: &PluginRegistry) -> Option<String> {
    let plugin = registry.get(env_name)?;
    let install_script = plugin.install_script.as_ref()?;

    let mut script = String::from(LAYER_CAPTURE_HEADER);
    script.push_str(install_script);

    // Copy tools to sandbox user (from plugin paths.copy)
    for cp in &plugin.manifest.paths.copy {
        if let Some(parent) = std::path::Path::new(&cp.to).parent() {
            script.push_str(&format!("mkdir -p {}\n", parent.display()));
        }
        script.push_str(&format!(
            "cp -rp {} {} 2>/dev/null || true\n",
            cp.from, cp.to
        ));
        script.push_str(&format!(
            "chown -R 1000:1000 {} 2>/dev/null || true\n",
            cp.to
        ));
    }

    // Ensure files in plugin bin directories are executable
    for bin in &plugin.manifest.paths.bin {
        script.push_str(&format!(
            "find {bin} -maxdepth 1 -type f -exec chmod a+x {{}} + 2>/dev/null || true\n"
        ));
    }

    script.push_str(LAYER_CAPTURE_FOOTER);
    Some(script)
}

/// Status of the layered cache for a given configuration.
pub struct LayerCacheStatus {
    /// Layers that have cached tarballs, in extraction order.
    pub cached: Vec<(String, PathBuf)>,
    /// Layer names that need building.
    pub uncached: Vec<String>,
}

impl LayerCacheStatus {
    pub fn all_cached(&self) -> bool {
        self.uncached.is_empty()
    }
}

/// Compute the script text used for a layer's version hash.
fn layer_script_text(
    layer_name: &str,
    shell: &SandboxShell,
    registry: &PluginRegistry,
) -> Option<String> {
    match layer_name {
        "base" => Some(build_base_layer_script(shell)),
        env_name => build_env_layer_script(env_name, registry),
    }
}

/// Check which layers are cached and which need building.
pub fn check_layer_cache(
    environments: &[String],
    shell: &SandboxShell,
    registry: &PluginRegistry,
) -> LayerCacheStatus {
    let mut cached = Vec::new();
    let mut uncached = Vec::new();

    // Determine all needed layers in order: base, then envs (sorted)
    let mut layer_names = vec!["base".to_string()];
    let mut sorted_envs: Vec<String> = environments.to_vec();
    sorted_envs.sort();
    layer_names.extend(sorted_envs);

    for name in &layer_names {
        if let Some(script) = layer_script_text(name, shell, registry) {
            let hash = layer_version_hash(&script);
            let path = paths::layer_cache_path(name, &hash, shell.name());
            if path.exists() {
                cached.push((name.clone(), path));
            } else {
                uncached.push(name.clone());
            }
        }
    }

    LayerCacheStatus { cached, uncached }
}

/// Build a fixup script that sets up combined PATH for all environments and fixes ownership.
pub fn build_layered_fixup_script(environments: &[String], registry: &PluginRegistry) -> String {
    let mut script = String::from("#!/bin/bash\nset -e\n");

    let mut path_parts: Vec<&str> = Vec::new();
    for env in environments {
        if let Some(plugin) = registry.get(env) {
            for bin in &plugin.manifest.paths.bin {
                path_parts.push(bin);
            }
        }
    }
    path_parts.push("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");

    script.push_str(&format!(
        "echo 'export PATH=\"{}\"' >> /home/sandbox/.bashrc\n",
        path_parts.join(":")
    ));

    // Root PATH from plugin bin paths
    let root_bins: Vec<String> = environments
        .iter()
        .filter_map(|e| registry.get(e))
        .flat_map(|p| p.manifest.paths.bin.iter())
        .filter_map(|b| {
            b.strip_prefix("/home/sandbox/")
                .map(|rest| format!("/root/{rest}"))
                .or_else(|| Some(b.clone()))
        })
        .collect();
    let root_path = if root_bins.is_empty() {
        "$PATH".to_string()
    } else {
        format!("{}:$PATH", root_bins.join(":"))
    };
    script.push_str(&format!(
        "echo 'export PATH=\"{root_path}\"' >> /root/.bashrc\n"
    ));

    script.push_str("chown -R 1000:1000 /home/sandbox/\n");
    script
}

/// Cache the base layer by tarballing the entire rootfs.
pub fn cache_base_layer(name: &str, shell: &SandboxShell) -> Result<PathBuf, IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    let script = build_base_layer_script(shell);
    let hash = layer_version_hash(&script);
    let cache_path = paths::layer_cache_path("base", &hash, shell.name());

    std::fs::create_dir_all(paths::layers_cache_dir())?;

    let file = std::fs::File::create(&cache_path)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    builder
        .append_dir_all(".", &rootfs_path)
        .map_err(|e| IsolaError::ExtractionFailed(format!("base layer cache: {e}")))?;
    builder
        .into_inner()
        .map_err(|e| IsolaError::ExtractionFailed(format!("base layer finalize: {e}")))?
        .finish()
        .map_err(|e| IsolaError::ExtractionFailed(format!("base layer compress: {e}")))?;

    Ok(cache_path)
}

/// Move the layer tarball created inside the sandbox to the cache directory.
pub fn cache_env_layer(
    name: &str,
    layer_name: &str,
    shell: &SandboxShell,
    registry: &PluginRegistry,
) -> Result<Option<PathBuf>, IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    let layer_tar_in_rootfs = rootfs_path.join("tmp/.layer_cache.tar.gz");

    if !layer_tar_in_rootfs.exists() {
        return Ok(None);
    }

    let script = layer_script_text(layer_name, shell, registry)
        .ok_or_else(|| IsolaError::PluginError(format!("unknown layer: {layer_name}")))?;
    let hash = layer_version_hash(&script);
    let cache_path = paths::layer_cache_path(layer_name, &hash, shell.name());

    std::fs::create_dir_all(paths::layers_cache_dir())?;
    std::fs::rename(&layer_tar_in_rootfs, &cache_path)?;

    Ok(Some(cache_path))
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

    // --- Layered cache tests ---

    #[test]
    fn layer_version_hash_deterministic() {
        let h1 = layer_version_hash("some script");
        let h2 = layer_version_hash("some script");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 12);
    }

    #[test]
    fn layer_version_hash_changes_with_content() {
        let h1 = layer_version_hash("script v1");
        let h2 = layer_version_hash("script v2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn base_layer_script_contains_essentials() {
        let script = build_base_layer_script(&SandboxShell::Bash);
        assert!(script.contains("apt-get update"));
        assert!(script.contains("Creating sandbox user"));
        assert!(script.contains("groupadd"));
        assert!(script.contains("/bin/bash"));
    }

    #[test]
    fn base_layer_script_shell_for_useradd() {
        let fish = build_base_layer_script(&SandboxShell::Fish);
        // Base layer sets correct shell for useradd but does NOT install it (plugin does)
        assert!(fish.contains("/usr/bin/fish"));
        assert!(!fish.contains("Installing fish shell"));

        let zsh = build_base_layer_script(&SandboxShell::Zsh);
        assert!(zsh.contains("/usr/bin/zsh"));
        assert!(!zsh.contains("Installing zsh"));
    }

    #[test]
    fn env_layer_script_rust() {
        let r = registry();
        let script = build_env_layer_script("rust", &r).unwrap();
        assert!(script.contains("Installing Rust"));
        assert!(script.contains("cp -rp /root/.cargo /home/sandbox/.cargo"));
        assert!(script.contains(".layer_marker"));
        assert!(script.contains(".layer_cache.tar.gz"));
    }

    #[test]
    fn env_layer_script_nodejs() {
        let r = registry();
        let script = build_env_layer_script("nodejs", &r).unwrap();
        assert!(script.contains("Installing Node.js"));
        assert!(script.contains(".layer_marker"));
    }

    #[test]
    fn env_layer_script_unknown_returns_none() {
        let r = registry();
        assert!(build_env_layer_script("unknown", &r).is_none());
    }

    #[test]
    fn env_layer_script_neovim() {
        let r = registry();
        let script = build_env_layer_script("neovim", &r).unwrap();
        assert!(script.contains("Installing neovim"));
        assert!(script.contains(".layer_marker"));
    }

    #[test]
    fn layered_fixup_script_empty_envs() {
        let r = registry();
        let script = build_layered_fixup_script(&[], &r);
        assert!(script.contains("chown -R 1000:1000 /home/sandbox/"));
        assert!(!script.contains("/home/sandbox/.cargo/bin"));
    }

    #[test]
    fn layered_fixup_script_with_envs() {
        let r = registry();
        let envs = vec!["rust".to_string(), "go".to_string()];
        let script = build_layered_fixup_script(&envs, &r);
        assert!(script.contains("/home/sandbox/.cargo/bin"));
        assert!(script.contains("/usr/local/go/bin"));
        assert!(!script.contains("/home/sandbox/.local/bin"));
    }

    #[test]
    fn check_layer_cache_reports_all_layers() {
        let r = registry();
        let envs = vec!["rust".to_string()];
        let status = check_layer_cache(&envs, &SandboxShell::Bash, &r);
        let total = status.cached.len() + status.uncached.len();
        assert_eq!(total, 2);
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
    fn check_layer_cache_sorts_envs() {
        let r = registry();
        let envs = vec!["nodejs".to_string(), "rust".to_string(), "go".to_string()];
        let status = check_layer_cache(&envs, &SandboxShell::Bash, &r);
        let mut all_names: Vec<String> = status
            .cached
            .iter()
            .map(|(n, _)| n.clone())
            .chain(status.uncached.iter().cloned())
            .collect();
        all_names.sort();
        assert_eq!(all_names, vec!["base", "go", "nodejs", "rust"]);
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
