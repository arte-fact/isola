#[cfg(target_os = "linux")]
use std::io::Read as _;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use crate::error::{IoContext, IsolaError};
#[cfg(target_os = "linux")]
use crate::paths;
use crate::plugin::{Plugin, PluginRegistry};
use crate::sandbox::config::SandboxShell;

#[cfg(target_os = "linux")]
const ROOTFS_URL: &str = "https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/ubuntu-base-24.04.4-base-amd64.tar.gz";
#[cfg(target_os = "linux")]
const ROOTFS_FILENAME: &str = "ubuntu-base-24.04.4-base-amd64.tar.gz";
#[cfg(target_os = "linux")]
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

// --- CLAUDE.md language-specific fragments (macOS/Lima only) ---

#[cfg(target_os = "macos")]
const CLAUDE_MD_RUST: &str = r#"
## Rust
- Build: `cargo build`, Test: `cargo test`, Lint: `cargo clippy -- -D warnings`, Format: `cargo fmt`
- Use `thiserror` for library error types and `anyhow` for application-level errors; propagate with `?`
- Never use `.unwrap()` in library code; use `.expect("reason")` only for true invariants
- Prefer borrowing (`&T`, `&mut T`) over taking ownership; use `Cow<'_, str>` for conditional ownership
- Use `Vec::with_capacity()` when the size is known; prefer `&str` over `String` where possible
- Organize imports: std → external crates → local modules; no wildcard imports except preludes
- Derive common traits (`Debug`, `Clone`, `PartialEq`) on public types
- Never commit `dbg!()` or `println!()` debug statements
- Run `cargo fmt` and `cargo clippy` before committing
"#;

#[cfg(target_os = "macos")]
const CLAUDE_MD_NODEJS: &str = r#"
## Node.js
- Use ES modules (`import`/`export`), not CommonJS (`require`)
- Destructure imports when possible: `import { foo } from 'bar'`
- Run `npm test` to run the test suite; prefer running single test files over the full suite for speed
- Use `npm run lint` or `npx eslint .` for linting; use `npx prettier --write .` for formatting
- Enable TypeScript strict mode (`"strict": true` in tsconfig.json) when using TypeScript
- Use `async`/`await` over raw Promises or callbacks
- Pin dependency versions in `package.json`; run `npm ci` for reproducible installs
- Never commit `node_modules/` or `.env` files
"#;

#[cfg(target_os = "macos")]
const CLAUDE_MD_PYTHON_UV: &str = r#"
## Python
- Use `uv` exclusively for package management — never use pip, pip-tools, poetry, or conda
- Install: `uv add <package>`, Remove: `uv remove <package>`, Sync: `uv sync`, Lock: `uv lock`
- Run scripts with `uv run <script>.py`; run tools with `uv run <tool>` (pytest, ruff, mypy)
- Launch a REPL with `uv run python`
- Use `uv run ruff check .` for linting and `uv run ruff format .` for formatting
- Use type hints on all function signatures; validate with `uv run mypy .`
- Use `uv run pytest` to run tests; prefer `uv run pytest path/to/test.py` for single files
- Never use bare `python` or `pip` commands — always go through `uv run`
"#;

#[cfg(target_os = "macos")]
const CLAUDE_MD_GO: &str = r#"
## Go
- Build: `go build ./...`, Test: `go test ./...`, Lint: `golangci-lint run` (if installed)
- Format with `gofmt` — code must always be gofmt-compliant
- Follow "accept interfaces, return structs" for flexible API design
- Use explicit error handling with return values; check every error, never discard with `_`
- Use `context.Context` as the first parameter for functions that do I/O or may be cancelled
- Prefer table-driven tests with `t.Run()` subtests
- Use `go vet ./...` before committing to catch common mistakes
- Standard project layout: `cmd/` for entrypoints, `internal/` for private packages, `pkg/` for public libraries
"#;

/// Build CLAUDE.md content based on selected environments (macOS/Lima only).
#[cfg(target_os = "macos")]
pub fn build_claude_md(environments: &[String], isolation_desc: &str) -> String {
    let mut md = format!(
        r#"# Sandbox Environment

You are running inside {isolation_desc}.

## Environment
- **OS**: Ubuntu 24.04 base"#
    );
    md.push_str(
        r#"
- **User**: sandbox (non-root)
- **Network**: Full unrestricted internet access (shared with host)
- **Workspace**: /workspace (bind-mounted from host project directory, read-write)

## Running privileged commands
Use sudo with the password "sandbox" for any privileged operation:
```
echo "sandbox" | sudo -S apt-get install -y <package>
echo "sandbox" | sudo -S <command>
```

## Available Tools
"#,
    );

    for env in environments {
        match env.as_str() {
            "rust" => md.push_str("- **Rust**: rustc + cargo (`/home/sandbox/.cargo/bin/`)\n"),
            "nodejs" => md.push_str("- **Node.js**: v22 LTS (`/usr/bin/node`, `/usr/bin/npm`)\n"),
            "python-uv" => md.push_str(
                "- **Python**: python3 + uv (`/usr/bin/python3`, `/home/sandbox/.local/bin/uv`)\n",
            ),
            "go" => md.push_str("- **Go**: (`/usr/local/go/bin/go`)\n"),
            _ => {}
        }
    }

    md.push_str("- **System**: use sudo to install any additional packages with apt-get\n");

    for env in environments {
        match env.as_str() {
            "rust" => md.push_str(CLAUDE_MD_RUST),
            "nodejs" => md.push_str(CLAUDE_MD_NODEJS),
            "python-uv" => md.push_str(CLAUDE_MD_PYTHON_UV),
            "go" => md.push_str(CLAUDE_MD_GO),
            _ => {}
        }
    }

    md.push_str(
        r#"
## Important
- Changes to `/workspace` are reflected on the host filesystem immediately.
- Changes outside `/workspace` persist across sandbox sessions (persistent sandbox).
- You cannot see or affect host processes. Your PID namespace is isolated.
- You are free to run any command without restriction.
"#,
    );

    md
}

#[cfg(target_os = "linux")]
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

/// Shell-escape a value for single-quoted bash literal.
fn sh_single_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', r"'\''"))
}

/// Emit `export VAR='value'` lines for a plugin's declared prompts, resolving
/// each value from `plugin_vars` first, then falling back to the prompt default.
pub fn emit_plugin_exports(
    plugin: &crate::plugin::Plugin,
    plugin_vars: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut out = String::new();
    for p in &plugin.manifest.prompts {
        let value = plugin_vars
            .get(&p.env_var)
            .cloned()
            .or_else(|| p.default.clone());
        if let Some(v) = value {
            out.push_str(&format!("export {}={}\n", p.env_var, sh_single_quote(&v)));
        }
    }
    out
}

/// Build a provisioning script from selected environments using the plugin registry.
pub fn build_provision_script(
    environments: &[String],
    shell: &SandboxShell,
    registry: &PluginRegistry,
    plugin_vars: &std::collections::BTreeMap<String, String>,
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

    // Resolve plugins once (warning on unknown environments); every step below
    // iterates this list instead of repeating the registry lookup.
    let plugins: Vec<&Plugin> = environments
        .iter()
        .filter_map(|env| match registry.get(env) {
            Some(p) => Some(p),
            None => {
                eprintln!("warning: unknown environment '{env}', skipping");
                None
            }
        })
        .collect();

    // Run each plugin's install script (shell plugins too; config-only skipped).
    for plugin in &plugins {
        if let Some(install_script) = &plugin.install_script {
            script.push('\n');
            script.push_str(&emit_plugin_exports(plugin, plugin_vars));
            script.push_str(install_script);
        }
    }

    // Copy plugin tools into the sandbox home (paths.copy) and gather bin dirs.
    let mut path_parts: Vec<&str> = Vec::new();
    for plugin in &plugins {
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

    script.push_str("chown -R 1000:1000 /home/sandbox/\n");

    // Ensure files directly in plugin bin directories are executable. Some
    // installers (e.g. Claude Code) create files without the execute bit.
    for bin in plugin_bin_dirs(&plugins) {
        script.push_str(&format!(
            "find {bin} -maxdepth 1 -type f -exec chmod a+x {{}} + 2>/dev/null || true\n"
        ));
    }

    // Sandbox user PATH.
    path_parts.push("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
    script.push_str(&format!(
        "echo 'export PATH=\"{}\"' >> /home/sandbox/.bashrc\n",
        path_parts.join(":")
    ));

    // Root PATH (rewrite /home/sandbox/* bin dirs to /root/*).
    let mut root_path_parts: Vec<String> = Vec::new();
    for bin in plugin_bin_dirs(&plugins) {
        match bin.strip_prefix("/home/sandbox/") {
            Some(rest) => root_path_parts.push(format!("/root/{rest}")),
            None => root_path_parts.push(bin.to_string()),
        }
    }
    root_path_parts.push("$PATH".to_string());
    script.push_str(&format!(
        "echo 'export PATH=\"{}\"' >> /root/.bashrc\n",
        root_path_parts.join(":")
    ));

    emit_verify(&mut script, &plugins);
    script
}

/// All `paths.bin` directories declared across the given plugins, in order.
fn plugin_bin_dirs<'a>(plugins: &'a [&Plugin]) -> impl Iterator<Item = &'a str> {
    plugins
        .iter()
        .flat_map(|p| p.manifest.paths.bin.iter().map(String::as_str))
}

/// Append the verification block: put plugin bin dirs on PATH so verify commands
/// resolve by name, then run each plugin's verify command.
fn emit_verify(script: &mut String, plugins: &[&Plugin]) {
    let mut verify_bins: Vec<&str> = Vec::new();
    for bin in plugin_bin_dirs(plugins) {
        if !verify_bins.contains(&bin) {
            verify_bins.push(bin);
        }
    }
    script.push_str("\necho \">>> Verifying...\"\n");
    if !verify_bins.is_empty() {
        script.push_str(&format!(
            "export PATH=\"{}:$PATH\"\n",
            verify_bins.join(":")
        ));
    }
    for plugin in plugins {
        if let Some(verify) = &plugin.manifest.provision.verify {
            script.push_str(verify);
            script.push('\n');
        }
    }
    script.push_str("id sandbox\n");
    script.push_str("echo \"=== Provisioning complete ===\"\n");
}

/// Download and cache rootfs with progress UI.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
/// Minimal SHA256 implementation (no external crate needed).
struct Sha256 {
    state: [u32; 8],
    buffer: Vec<u8>,
    total_len: u64,
}

#[cfg(target_os = "linux")]
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

        for (&k, &wi) in Self::K.iter().zip(w.iter()) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(k)
                .wrapping_add(wi);
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

#[cfg(target_os = "linux")]
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

    Ok(())
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
pub fn post_setup_rootfs(
    rootfs: &Path,
    name: &str,
    environments: &[String],
    registry: &PluginRegistry,
    fresh: bool,
) -> Result<(), IsolaError> {
    // Write a rootfs config file. On reprovision (`fresh == false`) the rootfs
    // already exists and provisioning has left many files owned by mapped
    // subordinate UIDs, so a host-side rewrite of e.g. /etc/* hits EACCES. Those
    // files persist correctly from the original create (and resolv.conf/hostname
    // are handled at runtime via bind-mount and sethostname), so a permission
    // error on reprovision is tolerated rather than fatal.
    let write_cfg = |path: &Path, contents: &str| -> Result<(), IsolaError> {
        let r = std::fs::write(path, contents);
        if !fresh && matches!(&r, Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied) {
            return Ok(());
        }
        r.io_ctx("write", path)
    };

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
    write_cfg(&resolv_dst, &host_resolv)?;

    let hostname_dst = rootfs.join("etc/hostname");
    write_cfg(&hostname_dst, &format!("{name}\n"))?;

    let hosts_dst = rootfs.join("etc/hosts");
    write_cfg(
        &hosts_dst,
        &format!("127.0.0.1 localhost {name}\n::1 localhost {name}\n"),
    )?;

    let apt_dir = rootfs.join("etc/apt/apt.conf.d");
    let _ = std::fs::create_dir_all(&apt_dir);
    let apt_cfg = apt_dir.join("99sandbox");
    write_cfg(&apt_cfg, "APT::Sandbox::User \"root\";\n")?;

    // Configure dpkg for user namespace environment
    let dpkg_dir = rootfs.join("etc/dpkg/dpkg.cfg.d");
    let _ = std::fs::create_dir_all(&dpkg_dir);
    let dpkg_cfg = dpkg_dir.join("01sandbox");
    write_cfg(&dpkg_cfg, "force-unsafe-io\n")?;

    for dir in &[
        "workspace",
        "root",
        // The sandbox user's home is created by `useradd -m` during provisioning,
        // but post_setup writes here first (git identity, host_copy), so ensure
        // it exists even when no host_copy plugin created it.
        "home/sandbox",
        "dev",
        "proc",
        "sys",
        "tmp",
        "usr/local/bin",
    ] {
        let d = rootfs.join(dir);
        std::fs::create_dir_all(&d).io_ctx("create dir", &d)?;
    }

    copy_host_files(rootfs, environments, registry, fresh)?;

    // Inherit host git identity (best-effort)
    if let Some(gitconfig) = build_host_gitconfig() {
        write_cfg(&rootfs.join("home/sandbox/.gitconfig"), &gitconfig)?;
        write_cfg(&rootfs.join("root/.gitconfig"), &gitconfig)?;
    }

    Ok(())
}

/// Copy host files declared by plugins (`paths.host_copy`) into the sandbox home.
///
/// On reprovision (`fresh == false`) a destination may already exist owned by a
/// mapped subordinate UID (a provision script wrote it), so a re-copy can hit
/// EACCES; the existing copy persists, so the error is tolerated.
fn copy_host_files(
    rootfs: &Path,
    environments: &[String],
    registry: &PluginRegistry,
    fresh: bool,
) -> Result<(), IsolaError> {
    let Some(home) = std::env::var("HOME").ok().map(PathBuf::from) else {
        return Ok(());
    };
    for env in environments {
        let Some(plugin) = registry.get(env) else {
            continue;
        };
        for entry in &plugin.manifest.paths.host_copy {
            let src = home.join(&entry.from);
            if !src.exists() {
                continue;
            }
            let dst = rootfs.join("home/sandbox").join(&entry.to);
            let res = if src.is_dir() {
                copy_dir_recursive(&src, &dst)
            } else {
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).io_ctx("create dir", parent)?;
                }
                std::fs::copy(&src, &dst).io_ctx("copy", &src).map(|_| ())
            };
            match res {
                Ok(()) => {}
                Err(_) if !fresh => { /* keep existing copy on reprovision */ }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn rootfs_url() -> &'static str {
    ROOTFS_URL
}

/// Check if a cached provisioned rootfs exists for the given environments.
#[cfg(target_os = "linux")]
pub fn has_cached_provision(environments: &[String], shell: &str) -> Option<PathBuf> {
    let path = paths::provision_cache_path(environments, shell);
    if path.exists() { Some(path) } else { None }
}

#[cfg(target_os = "linux")]
const PROVISION_CACHE_FILE: &str = ".provision_cache.tar.gz";
#[cfg(target_os = "linux")]
const BASE_CACHE_FILE: &str = ".base_cache.tar.gz";

#[cfg(target_os = "linux")]
fn build_full_rootfs_cache_script(out: &str) -> String {
    let stem = out.trim_end_matches(".tar.gz");
    // The tarball is written to /var/tmp (a real, persistent rootfs directory)
    // rather than /tmp, which is a fresh tmpfs at runtime — a tarball written
    // there would vanish when the namespace exits. /var/tmp is excluded from the
    // archive so the in-progress tarball isn't captured in itself, and it is
    // owned by the host user (from rootfs extraction) so the host-side move out
    // succeeds despite the sticky bit.
    format!(
        r#"#!/bin/bash
set -eo pipefail
echo ">>> Capturing rootfs cache..."
mkdir -p /var/tmp
rm -f /var/tmp/{out} /var/tmp/{stem}.partial
cd /
tar czf /var/tmp/{stem}.partial \
    --exclude=./proc/* \
    --exclude=./sys/* \
    --exclude=./dev/* \
    --exclude=./tmp/* \
    --exclude=./run/* \
    --exclude=./var/cache/apt/archives/* \
    --exclude=./var/tmp/* \
    --exclude=./workspace/* \
    .
mv /var/tmp/{stem}.partial /var/tmp/{out}
[ -s /var/tmp/{out} ] || {{ echo "error: cache tarball is empty" >&2; exit 1; }}
echo "=== Rootfs cache complete ==="
"#
    )
}

#[cfg(target_os = "linux")]
pub fn build_provision_cache_script() -> String {
    build_full_rootfs_cache_script(PROVISION_CACHE_FILE)
}

#[cfg(target_os = "linux")]
pub fn build_base_cache_script() -> String {
    build_full_rootfs_cache_script(BASE_CACHE_FILE)
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
pub fn build_fixup_script() -> String {
    "#!/bin/bash\nset -e\nchown -R 1000:1000 /home/sandbox/\n".to_string()
}

// --- Layered cache support ---

/// Compute a short hash of a provisioning script fragment for cache keying.
/// Returns the first 12 hex characters of SHA256(script_text).
#[cfg(target_os = "linux")]
pub fn layer_version_hash(script_text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(script_text.as_bytes());
    hasher.finish_hex()[..12].to_string()
}

#[cfg(target_os = "linux")]
/// Script fragment for capturing layer diff via marker file.
const LAYER_CAPTURE_HEADER: &str = r#"#!/bin/bash
set -eo pipefail
export DEBIAN_FRONTEND=noninteractive
export PATH="/root/.cargo/bin:/root/.local/bin:/usr/local/go/bin:$PATH"
touch /tmp/.layer_marker
sleep 1
"#;

#[cfg(target_os = "linux")]
const LAYER_CAPTURE_FOOTER: &str = r#"
echo ">>> Capturing layer..."
mkdir -p /var/tmp
find / -xdev -newer /tmp/.layer_marker \
    ! -path '/proc/*' ! -path '/sys/*' ! -path '/dev/*' ! -path '/tmp/*' \
    ! -path '/var/tmp/*' ! -path '/var/cache/apt/archives/*' ! -path '/workspace/*' ! -path '/run/*' \
    -print0 2>/dev/null | \
    tar czf /var/tmp/.layer_cache.tar.gz --null -T - 2>/dev/null || true
echo "=== Layer complete ==="
"#;

/// Build a provisioning script for the base layer (packages + shell + user creation).
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
pub fn build_env_layer_script(
    env_name: &str,
    registry: &PluginRegistry,
    plugin_vars: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let plugin = registry.get(env_name)?;
    let install_script = plugin.install_script.as_ref()?;

    let mut script = String::from(LAYER_CAPTURE_HEADER);
    let exports = emit_plugin_exports(plugin, plugin_vars);
    if !exports.is_empty() {
        script.push_str(&exports);
    }
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

#[cfg(target_os = "linux")]
/// Status of the layered cache for a given configuration.
pub struct LayerCacheStatus {
    /// Layers that have cached tarballs, in extraction order.
    pub cached: Vec<(String, PathBuf)>,
    /// Layer names that need building.
    pub uncached: Vec<String>,
}

#[cfg(target_os = "linux")]
impl LayerCacheStatus {
    pub fn all_cached(&self) -> bool {
        self.uncached.is_empty()
    }

    /// Env layers are diffs captured on top of a provisioned base (no /bin/bash
    /// of their own). If base is uncached — e.g. the shell differs from a
    /// prior sandbox, since only base layers are keyed by shell — the cached
    /// env diffs can't stand on their own and must be rebuilt too.
    fn invalidate_envs_if_base_missing(&mut self) {
        if self.uncached.iter().any(|n| n == "base") {
            for (name, _) in self.cached.drain(..) {
                self.uncached.push(name);
            }
        }
    }
}

/// Compute the script text used for a layer's version hash.
#[cfg(target_os = "linux")]
fn layer_script_text(
    layer_name: &str,
    shell: &SandboxShell,
    registry: &PluginRegistry,
    plugin_vars: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    match layer_name {
        "base" => Some(build_base_layer_script(shell)),
        env_name => build_env_layer_script(env_name, registry, plugin_vars),
    }
}

/// Check which layers are cached and which need building.
#[cfg(target_os = "linux")]
pub fn check_layer_cache(
    environments: &[String],
    shell: &SandboxShell,
    registry: &PluginRegistry,
    plugin_vars: &std::collections::BTreeMap<String, String>,
) -> LayerCacheStatus {
    let mut cached = Vec::new();
    let mut uncached = Vec::new();

    // Determine all needed layers in order: base, then envs (sorted)
    let mut layer_names = vec!["base".to_string()];
    let mut sorted_envs: Vec<String> = environments.to_vec();
    sorted_envs.sort();
    layer_names.extend(sorted_envs);

    for name in &layer_names {
        if let Some(script) = layer_script_text(name, shell, registry, plugin_vars) {
            let hash = layer_version_hash(&script);
            let path = paths::layer_cache_path(name, &hash, shell.name());
            if path.exists() {
                cached.push((name.clone(), path));
            } else {
                uncached.push(name.clone());
            }
        }
    }

    let mut status = LayerCacheStatus { cached, uncached };
    status.invalidate_envs_if_base_missing();
    status
}

/// Build a fixup script that sets up combined PATH for all environments and fixes ownership.
#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
pub fn cache_base_layer(name: &str, shell: &SandboxShell) -> Result<PathBuf, IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    let script = build_base_layer_script(shell);
    let hash = layer_version_hash(&script);
    let cache_path = paths::layer_cache_path("base", &hash, shell.name());
    let src = rootfs_path.join("var/tmp").join(BASE_CACHE_FILE);
    move_cache_tarball(&src, &cache_path)?;
    Ok(cache_path)
}

/// Move the layer tarball created inside the sandbox to the cache directory.
#[cfg(target_os = "linux")]
pub fn cache_env_layer(
    name: &str,
    layer_name: &str,
    shell: &SandboxShell,
    registry: &PluginRegistry,
    plugin_vars: &std::collections::BTreeMap<String, String>,
) -> Result<Option<PathBuf>, IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    let layer_tar_in_rootfs = rootfs_path.join("var/tmp/.layer_cache.tar.gz");

    if !layer_tar_in_rootfs.exists() {
        return Ok(None);
    }

    let script = layer_script_text(layer_name, shell, registry, plugin_vars)
        .ok_or_else(|| IsolaError::PluginError(format!("unknown layer: {layer_name}")))?;
    let hash = layer_version_hash(&script);
    let cache_path = paths::layer_cache_path(layer_name, &hash, shell.name());

    let layers_dir = paths::layers_cache_dir();
    std::fs::create_dir_all(&layers_dir).io_ctx("create layers cache dir", &layers_dir)?;
    std::fs::rename(&layer_tar_in_rootfs, &cache_path)
        .io_ctx("rename layer tarball into cache", &layer_tar_in_rootfs)?;

    Ok(Some(cache_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginRegistry;
    use std::collections::BTreeMap;

    fn registry() -> PluginRegistry {
        PluginRegistry::load().unwrap()
    }

    fn pv() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn provision_script_base_only() {
        let r = registry();
        let script = build_provision_script(&[], &SandboxShell::bash(), &r, &pv());
        assert!(script.contains("apt-get update"));
        assert!(script.contains("Creating sandbox user"));
        assert!(!script.contains("Installing Rust"));
        assert!(!script.contains("Installing Node"));
    }

    #[test]
    fn provision_script_includes_selected_envs() {
        let r = registry();
        let envs = vec!["rust".to_string(), "nodejs".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::bash(), &r, &pv());
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
        let script = build_provision_script(&envs, &SandboxShell::bash(), &r, &pv());
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
        let script = build_provision_script(&envs, &SandboxShell::bash(), &r, &pv());
        assert!(script.contains("/home/sandbox/.cargo/bin"));
        assert!(script.contains("cp -rp /root/.cargo /home/sandbox/.cargo"));
    }

    #[test]
    fn provision_script_unknown_env_ignored() {
        let r = registry();
        let envs = vec!["unknown-env".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::bash(), &r, &pv());
        assert!(script.contains("apt-get update"));
    }

    #[test]
    fn provision_script_installs_fish() {
        let r = registry();
        let envs = vec!["fish".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::fish(), &r, &pv());
        assert!(script.contains("Installing fish shell"));
        assert!(script.contains("/usr/bin/fish"));
    }

    #[test]
    fn provision_script_installs_zsh() {
        let r = registry();
        let envs = vec!["zsh".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::zsh(), &r, &pv());
        assert!(script.contains("Installing zsh"));
        assert!(script.contains("/usr/bin/zsh"));
    }

    #[test]
    fn provision_script_installs_neovim() {
        let r = registry();
        let envs = vec!["neovim".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::bash(), &r, &pv());
        assert!(script.contains("Installing neovim"));
    }

    // --- Layered cache tests (Linux only) ---

    #[cfg(target_os = "linux")]
    #[test]
    fn layer_version_hash_deterministic() {
        let h1 = layer_version_hash("some script");
        let h2 = layer_version_hash("some script");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 12);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn layer_version_hash_changes_with_content() {
        let h1 = layer_version_hash("script v1");
        let h2 = layer_version_hash("script v2");
        assert_ne!(h1, h2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn base_layer_script_contains_essentials() {
        let script = build_base_layer_script(&SandboxShell::bash());
        assert!(script.contains("apt-get update"));
        assert!(script.contains("Creating sandbox user"));
        assert!(script.contains("groupadd"));
        assert!(script.contains("/bin/bash"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn base_layer_script_shell_for_useradd() {
        let fish = build_base_layer_script(&SandboxShell::fish());
        // Base layer sets correct shell for useradd but does NOT install it (plugin does)
        assert!(fish.contains("/usr/bin/fish"));
        assert!(!fish.contains("Installing fish shell"));

        let zsh = build_base_layer_script(&SandboxShell::zsh());
        assert!(zsh.contains("/usr/bin/zsh"));
        assert!(!zsh.contains("Installing zsh"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn env_layer_script_rust() {
        let r = registry();
        let script = build_env_layer_script("rust", &r, &pv()).unwrap();
        assert!(script.contains("Installing Rust"));
        assert!(script.contains("cp -rp /root/.cargo /home/sandbox/.cargo"));
        assert!(script.contains(".layer_marker"));
        assert!(script.contains(".layer_cache.tar.gz"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn env_layer_script_nodejs() {
        let r = registry();
        let script = build_env_layer_script("nodejs", &r, &pv()).unwrap();
        assert!(script.contains("Installing Node.js"));
        assert!(script.contains(".layer_marker"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn env_layer_script_unknown_returns_none() {
        let r = registry();
        assert!(build_env_layer_script("unknown", &r, &pv()).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn env_layer_script_neovim() {
        let r = registry();
        let script = build_env_layer_script("neovim", &r, &pv()).unwrap();
        assert!(script.contains("Installing neovim"));
        assert!(script.contains(".layer_marker"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn layered_fixup_script_empty_envs() {
        let r = registry();
        let script = build_layered_fixup_script(&[], &r);
        assert!(script.contains("chown -R 1000:1000 /home/sandbox/"));
        assert!(!script.contains("/home/sandbox/.cargo/bin"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn layered_fixup_script_with_envs() {
        let r = registry();
        let envs = vec!["rust".to_string(), "go".to_string()];
        let script = build_layered_fixup_script(&envs, &r);
        assert!(script.contains("/home/sandbox/.cargo/bin"));
        assert!(script.contains("/usr/local/go/bin"));
        assert!(!script.contains("/home/sandbox/.local/bin"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn check_layer_cache_reports_all_layers() {
        let r = registry();
        let envs = vec!["rust".to_string()];
        let status = check_layer_cache(&envs, &SandboxShell::bash(), &r, &pv());
        let total = status.cached.len() + status.uncached.len();
        assert_eq!(total, 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn provision_script_creates_user_before_plugins() {
        let r = registry();
        let envs = vec!["gpu".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::bash(), &r, &pv());
        let user_pos = script.find("Creating sandbox user").unwrap();
        let gpu_pos = script.find("Setting up GPU access").unwrap();
        assert!(
            user_pos < gpu_pos,
            "sandbox user must be created before GPU plugin runs"
        );
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn ensure_rootfs_has_bash_errors_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let err = ensure_rootfs_has_bash(tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no working /bin/bash"), "got: {msg}");
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn provision_cache_script_uses_partial_then_rename() {
        let s = build_provision_cache_script();
        // Atomic: write to .partial, then mv into place — never leave a
        // truncated .tar.gz that future runs would treat as a valid cache.
        assert!(s.contains(".provision_cache.partial"));
        assert!(
            s.contains("mv /var/tmp/.provision_cache.partial /var/tmp/.provision_cache.tar.gz")
        );
        // The tarball must live on a persistent rootfs path (/var/tmp), not the
        // tmpfs /tmp where it would vanish when the namespace exits.
        assert!(s.contains("tar czf /var/tmp/.provision_cache.partial"));
        // Fail-fast on any error so a partial tarball doesn't get renamed.
        assert!(s.contains("set -eo pipefail"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cache_script_excludes_virtual_filesystems_and_workspace() {
        // /proc /sys /dev /run are kernel virtual fs, /tmp would recurse into
        // the tarball itself, /workspace is a host bind-mount that must never
        // end up in the cached image.
        for s in [build_provision_cache_script(), build_base_cache_script()] {
            for excl in [
                "./proc/*",
                "./sys/*",
                "./dev/*",
                "./tmp/*",
                "./run/*",
                // The scratch dir holding the in-progress tarball itself.
                "./var/tmp/*",
                // The shared apt archives cache (bind-mounted during provisioning).
                "./var/cache/apt/archives/*",
                "./workspace/*",
            ] {
                assert!(s.contains(excl), "missing exclusion {excl} in:\n{s}");
            }
        }
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn invalidate_envs_when_base_missing_moves_envs_to_uncached() {
        let mut status = LayerCacheStatus {
            cached: vec![
                ("rust".to_string(), PathBuf::from("/fake/env-rust.tar.gz")),
                (
                    "nodejs".to_string(),
                    PathBuf::from("/fake/env-nodejs.tar.gz"),
                ),
            ],
            uncached: vec!["base".to_string()],
        };
        status.invalidate_envs_if_base_missing();
        assert!(status.cached.is_empty());
        assert_eq!(status.uncached, vec!["base", "rust", "nodejs"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn invalidate_envs_when_base_cached_keeps_envs_cached() {
        let mut status = LayerCacheStatus {
            cached: vec![
                ("base".to_string(), PathBuf::from("/fake/base.tar.gz")),
                ("rust".to_string(), PathBuf::from("/fake/env-rust.tar.gz")),
            ],
            uncached: vec!["nodejs".to_string()],
        };
        status.invalidate_envs_if_base_missing();
        assert_eq!(status.cached.len(), 2);
        assert_eq!(status.uncached, vec!["nodejs"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn check_layer_cache_sorts_envs() {
        let r = registry();
        let envs = vec!["nodejs".to_string(), "rust".to_string(), "go".to_string()];
        let status = check_layer_cache(&envs, &SandboxShell::bash(), &r, &pv());
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
    #[cfg(target_os = "linux")]
    #[test]
    fn sha256_empty_string() {
        let mut h = Sha256::new();
        h.update(b"");
        assert_eq!(
            h.finish_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sha256_abc() {
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(
            h.finish_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sha256_long_message() {
        let mut h = Sha256::new();
        h.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            h.finish_hex(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[cfg(target_os = "linux")]
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
