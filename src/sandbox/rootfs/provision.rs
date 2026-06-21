//! Build the provisioning script for a sandbox and configure a freshly
//! extracted rootfs (DNS, hostname, host file imports, git identity).

#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use crate::error::{IoContext, IsolaError};
use crate::plugin::{Plugin, PluginRegistry};
use crate::sandbox::config::SandboxShell;

/// Base packages installed in every sandbox. `pub(crate)` so the layered-cache
/// base-layer script can reuse it.
pub(crate) const PROVISION_BASE: &str = r#"
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
#[cfg(target_os = "linux")]
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
    plugin: &Plugin,
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

/// Configure a freshly extracted rootfs: DNS, hostname/hosts, apt/dpkg tweaks,
/// base directories, host file imports, and git identity. On reprovision
/// (`fresh == false`) host-side writes that hit EACCES on subordinate-UID-owned
/// files are tolerated, since the original create already wrote them.
#[cfg(target_os = "linux")]
pub fn post_setup_rootfs(
    rootfs: &Path,
    name: &str,
    environments: &[String],
    registry: &PluginRegistry,
    fresh: bool,
) -> Result<(), IsolaError> {
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
#[cfg(target_os = "linux")]
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
}
