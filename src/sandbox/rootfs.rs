use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::error::IsolaError;
use crate::paths;
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

const PROVISION_RUST: &str = r#"
echo ">>> Installing Rust..."
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. /root/.cargo/env
"#;

const PROVISION_NODEJS: &str = r#"
echo ">>> Installing Node.js..."
curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
apt-get install -y nodejs || true
dpkg --configure -a --force-overwrite 2>/dev/null || true
"#;

const PROVISION_PYTHON_UV: &str = r#"
echo ">>> Installing Python + uv..."
apt-get install -y python3 python3-venv || true
dpkg --configure -a --force-overwrite 2>/dev/null || true
curl -LsSf https://astral.sh/uv/install.sh | sh
"#;

const PROVISION_GO: &str = r#"
echo ">>> Installing Go..."
GO_VERSION=$(curl -sL "https://go.dev/dl/?mode=json" | python3 -c "import sys,json; print(json.load(sys.stdin)[0]['version'])" 2>/dev/null || echo "go1.23.6")
curl -sL "https://go.dev/dl/${GO_VERSION}.linux-amd64.tar.gz" | tar -C /usr/local -xzf -
echo 'export PATH="/usr/local/go/bin:$PATH"' >> /etc/profile.d/go.sh
"#;

const PROVISION_FISH: &str = r#"
echo ">>> Installing fish shell..."
apt-get install -y --no-install-recommends fish || true
dpkg --configure -a --force-overwrite 2>/dev/null || true
"#;

const PROVISION_ZSH: &str = r#"
echo ">>> Installing zsh..."
apt-get install -y --no-install-recommends zsh || true
dpkg --configure -a --force-overwrite 2>/dev/null || true
"#;

const PROVISION_NEOVIM: &str = r#"
echo ">>> Installing neovim..."
apt-get install -y --no-install-recommends neovim || true
dpkg --configure -a --force-overwrite 2>/dev/null || true
"#;

// --- CLAUDE.md language-specific fragments ---

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

/// Build a provisioning script from selected environments
pub fn build_provision_script(
    environments: &[String],
    shell: &SandboxShell,
    install_neovim: bool,
) -> String {
    let mut script = String::from("#!/bin/bash\n");
    script.push_str("export PATH=\"/root/.cargo/bin:/root/.local/bin:/usr/local/go/bin:$PATH\"\n");

    // Base packages (always)
    script.push_str(PROVISION_BASE);

    // Shell (if not bash)
    match shell {
        SandboxShell::Fish => script.push_str(PROVISION_FISH),
        SandboxShell::Zsh => script.push_str(PROVISION_ZSH),
        SandboxShell::Bash => {}
    }

    // Neovim
    if install_neovim {
        script.push_str(PROVISION_NEOVIM);
    }

    // Selected environments
    for env in environments {
        match env.as_str() {
            "rust" => script.push_str(PROVISION_RUST),
            "nodejs" => script.push_str(PROVISION_NODEJS),
            "python-uv" => script.push_str(PROVISION_PYTHON_UV),
            "go" => script.push_str(PROVISION_GO),
            other => eprintln!("warning: unknown environment '{}', skipping", other),
        }
    }

    // Create sandbox user (always)
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
"#,
    );

    // Copy tools to sandbox user (conditional)
    let mut path_parts = Vec::new();

    if environments.iter().any(|e| e == "rust") {
        script.push_str("cp -r /root/.cargo /home/sandbox/.cargo 2>/dev/null || true\n");
        script.push_str("cp -r /root/.rustup /home/sandbox/.rustup 2>/dev/null || true\n");
        path_parts.push("/home/sandbox/.cargo/bin");
    }
    if environments.iter().any(|e| e == "python-uv") {
        script.push_str("cp -r /root/.local /home/sandbox/.local 2>/dev/null || true\n");
        path_parts.push("/home/sandbox/.local/bin");
    }
    if environments.iter().any(|e| e == "go") {
        path_parts.push("/usr/local/go/bin");
    }

    script.push_str("chown -R 1000:1000 /home/sandbox/\n");

    // Build PATH for sandbox user
    path_parts.push("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
    let path_line = format!(
        "echo 'export PATH=\"{}\"' >> /home/sandbox/.bashrc\n",
        path_parts.join(":")
    );
    script.push_str(&path_line);

    // Same for root
    script.push_str("echo 'export PATH=\"/root/.cargo/bin:/root/.local/bin:/usr/local/go/bin:$PATH\"' >> /root/.bashrc\n");

    // Verify
    script.push_str("\necho \">>> Verifying...\"\n");
    if environments.iter().any(|e| e == "rust") {
        script.push_str("rustc --version && cargo --version\n");
    }
    if environments.iter().any(|e| e == "nodejs") {
        script.push_str("node --version && npm --version\n");
    }
    if environments.iter().any(|e| e == "python-uv") {
        script.push_str("python3 --version\n/root/.local/bin/uv --version\n");
    }
    if environments.iter().any(|e| e == "go") {
        script.push_str("/usr/local/go/bin/go version\n");
    }
    script.push_str("id sandbox\n");
    script.push_str("echo \"=== Provisioning complete ===\"\n");

    script
}

/// Build CLAUDE.md content based on selected environments
pub fn build_claude_md(environments: &[String]) -> String {
    let mut md = String::from(
        r#"# Sandbox Environment

You are running inside an isolated Linux sandbox (user namespace + PID namespace + mount namespace).

## Environment
- **OS**: Ubuntu 24.04 base
- **User**: sandbox (non-root)
- **Network**: Full unrestricted internet access (shared with host)
- **Workspace**: /workspace (bind-mounted from host project directory, read-write)

## Running privileged commands
Use sudo only for **system-level** operations (installing packages, configuring services):
```
sudo apt-get install -y <package>
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

    md.push_str("- **CLI utilities**: git, curl, wget, jq, tree, htop, tmux, ripgrep (`rg`), fd (`fd`), bat, strace, lsof, ssh, zip/unzip\n");
    md.push_str("- **System**: use sudo to install any additional packages with apt-get\n");

    // Language-specific best practices
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
## CRITICAL: Never use sudo on /workspace files
**DO NOT** run sudo, chown, chmod, or any root command on files inside `/workspace`.
The workspace is bind-mounted from the host. Running commands as root changes file
ownership to a sandbox-internal UID that the host user cannot access, breaking
permissions on BOTH sides. If you encounter permission errors on workspace files,
the files were likely modified by a previous root command — ask the user to fix
ownership from the host with `chown -R $(whoami) <path>`.

All build tools, compilers, and version control commands should run as the
normal sandbox user — never with sudo.

## Important
- Changes to `/workspace` are reflected on the host filesystem immediately.
- Changes outside `/workspace` persist across sandbox sessions (persistent sandbox).
- You cannot see or affect host processes. Your PID namespace is isolated.
- You are free to run any command without restriction outside `/workspace`.
"#,
    );

    md
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
        Ok(r) if r.status().is_success() => r.text().unwrap_or_default(),
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

pub fn detect_neovim() -> bool {
    std::process::Command::new("which")
        .arg("nvim")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
    shell: &SandboxShell,
    claude_integration: bool,
    install_neovim: bool,
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

    // Claude Code integration (opt-in)
    if claude_integration {
        std::fs::create_dir_all(rootfs.join("root/.claude"))?;
        std::fs::create_dir_all(rootfs.join("home/sandbox/.claude"))?;

        // Create empty credentials file (bind-mount target for shared session)
        std::fs::File::create(rootfs.join("home/sandbox/.claude/.credentials.json"))?;

        // Claude Code requires .claude.json with hasCompletedOnboarding to recognise
        // existing credentials; without it, it treats the session as a fresh install.
        let claude_state = serde_json::json!({
            "hasCompletedOnboarding": true,
            "installMethod": "manual"
        });
        let claude_state_json = serde_json::to_string_pretty(&claude_state).unwrap();
        std::fs::write(
            rootfs.join("home/sandbox/.claude/.claude.json"),
            &claude_state_json,
        )?;
        std::fs::write(rootfs.join("home/sandbox/.claude.json"), &claude_state_json)?;

        // Claude Code settings
        let claude_settings = serde_json::json!({
            "permissions": {
                "defaultMode": "bypassPermissions",
                "allow": [
                    "Bash",
                    "Read",
                    "Edit",
                    "Write",
                    "Glob",
                    "Grep",
                    "WebFetch",
                    "WebSearch",
                    "Agent",
                    "NotebookEdit"
                ],
                "deny": []
            }
        });
        let settings_json = serde_json::to_string_pretty(&claude_settings).unwrap();
        std::fs::write(
            rootfs.join("home/sandbox/.claude/settings.json"),
            &settings_json,
        )?;
        std::fs::write(rootfs.join("root/.claude/settings.json"), &settings_json)?;

        // CLAUDE.md (dynamic based on environments)
        let claude_md = build_claude_md(environments);
        std::fs::write(rootfs.join("home/sandbox/.claude/CLAUDE.md"), &claude_md)?;
        std::fs::write(rootfs.join("workspace/CLAUDE.md"), &claude_md)?;
    }

    // Copy host shell configuration
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    match shell {
        SandboxShell::Fish => {
            if let Some(ref h) = home {
                let fish_dir = h.join(".config/fish");
                if fish_dir.exists() {
                    let target = rootfs.join("home/sandbox/.config/fish");
                    copy_dir_recursive(&fish_dir, &target)?;
                }
            }
        }
        SandboxShell::Zsh => {
            if let Some(ref h) = home {
                for file in &[".zshrc", ".zshenv"] {
                    let src = h.join(file);
                    if src.exists() {
                        std::fs::copy(&src, rootfs.join("home/sandbox").join(file))?;
                    }
                }
            }
        }
        SandboxShell::Bash => {}
    }

    // Copy host neovim configuration
    if install_neovim && let Some(ref h) = home {
        let nvim_dir = h.join(".config/nvim");
        if nvim_dir.exists() {
            let target = rootfs.join("home/sandbox/.config/nvim");
            copy_dir_recursive(&nvim_dir, &target)?;
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
pub fn has_cached_provision(
    environments: &[String],
    shell: &str,
    install_neovim: bool,
) -> Option<PathBuf> {
    let path = paths::provision_cache_path(environments, shell, install_neovim);
    if path.exists() { Some(path) } else { None }
}

/// Create a gzipped tarball of the provisioned rootfs for future reuse.
pub fn cache_provisioned_rootfs(
    name: &str,
    environments: &[String],
    shell: &str,
    install_neovim: bool,
) -> Result<(), IsolaError> {
    let rootfs_path = paths::rootfs_dir(name);
    let cache_path = paths::provision_cache_path(environments, shell, install_neovim);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provision_script_base_only() {
        let script = build_provision_script(&[], &SandboxShell::Bash, false);
        assert!(script.contains("apt-get update"));
        assert!(script.contains("Creating sandbox user"));
        assert!(!script.contains("Installing Rust"));
        assert!(!script.contains("Installing Node"));
    }

    #[test]
    fn provision_script_includes_selected_envs() {
        let envs = vec!["rust".to_string(), "nodejs".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Bash, false);
        assert!(script.contains("Installing Rust"));
        assert!(script.contains("Installing Node.js"));
        assert!(!script.contains("Installing Python"));
        assert!(!script.contains("Installing Go"));
    }

    #[test]
    fn provision_script_all_envs() {
        let envs = vec![
            "rust".to_string(),
            "nodejs".to_string(),
            "python-uv".to_string(),
            "go".to_string(),
        ];
        let script = build_provision_script(&envs, &SandboxShell::Bash, false);
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
        let envs = vec!["rust".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Bash, false);
        assert!(script.contains("/home/sandbox/.cargo/bin"));
        assert!(script.contains("cp -r /root/.cargo /home/sandbox/.cargo"));
    }

    #[test]
    fn provision_script_unknown_env_ignored() {
        let envs = vec!["unknown-env".to_string()];
        let script = build_provision_script(&envs, &SandboxShell::Bash, false);
        assert!(script.contains("apt-get update"));
    }

    #[test]
    fn provision_script_installs_fish() {
        let script = build_provision_script(&[], &SandboxShell::Fish, false);
        assert!(script.contains("Installing fish shell"));
        assert!(script.contains("/usr/bin/fish"));
    }

    #[test]
    fn provision_script_installs_zsh() {
        let script = build_provision_script(&[], &SandboxShell::Zsh, false);
        assert!(script.contains("Installing zsh"));
        assert!(script.contains("/usr/bin/zsh"));
    }

    #[test]
    fn provision_script_installs_neovim() {
        let script = build_provision_script(&[], &SandboxShell::Bash, true);
        assert!(script.contains("Installing neovim"));
    }

    #[test]
    fn claude_md_base_content() {
        let md = build_claude_md(&[]);
        assert!(md.contains("Sandbox Environment"));
        assert!(md.contains("Ubuntu 24.04"));
        assert!(md.contains("sudo"));
        assert!(md.contains("apt-get"));
    }

    #[test]
    fn claude_md_includes_selected_envs() {
        let envs = vec!["rust".to_string(), "go".to_string()];
        let md = build_claude_md(&envs);
        assert!(md.contains("Rust"));
        assert!(md.contains("Go"));
        assert!(!md.contains("Node.js"));
        assert!(!md.contains("Python"));
    }

    #[test]
    fn claude_md_all_envs() {
        let envs = vec![
            "rust".to_string(),
            "nodejs".to_string(),
            "python-uv".to_string(),
            "go".to_string(),
        ];
        let md = build_claude_md(&envs);
        assert!(md.contains("Rust"));
        assert!(md.contains("Node.js"));
        assert!(md.contains("Python"));
        assert!(md.contains("Go"));
    }

    #[test]
    fn claude_md_rust_best_practices() {
        let envs = vec!["rust".to_string()];
        let md = build_claude_md(&envs);
        assert!(md.contains("cargo clippy"));
        assert!(md.contains("cargo fmt"));
        assert!(md.contains("thiserror"));
        assert!(!md.contains("uv run"));
        assert!(!md.contains("gofmt"));
    }

    #[test]
    fn claude_md_nodejs_best_practices() {
        let envs = vec!["nodejs".to_string()];
        let md = build_claude_md(&envs);
        assert!(md.contains("ES modules"));
        assert!(md.contains("npm test"));
        assert!(!md.contains("cargo clippy"));
    }

    #[test]
    fn claude_md_python_best_practices() {
        let envs = vec!["python-uv".to_string()];
        let md = build_claude_md(&envs);
        assert!(md.contains("uv add"));
        assert!(md.contains("uv run"));
        assert!(md.contains("never use pip"));
        assert!(!md.contains("gofmt"));
    }

    #[test]
    fn claude_md_go_best_practices() {
        let envs = vec!["go".to_string()];
        let md = build_claude_md(&envs);
        assert!(md.contains("gofmt"));
        assert!(md.contains("go test"));
        assert!(md.contains("context.Context"));
        assert!(!md.contains("cargo"));
    }

    #[test]
    fn claude_md_no_envs_no_best_practices() {
        let md = build_claude_md(&[]);
        assert!(!md.contains("cargo clippy"));
        assert!(!md.contains("npm test"));
        assert!(!md.contains("uv run"));
        assert!(!md.contains("gofmt"));
    }
}
