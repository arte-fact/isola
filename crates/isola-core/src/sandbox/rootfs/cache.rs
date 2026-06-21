//! Provision caching: the legacy monolithic provisioned-rootfs tarball and the
//! layered (base + per-environment) cache used to skip re-provisioning.

use std::path::{Path, PathBuf};

use crate::error::{IoContext, IsolaError};
use crate::paths;
use crate::plugin::PluginRegistry;
use crate::sandbox::config::SandboxShell;

use super::provision::{PROVISION_BASE, emit_plugin_exports};

const PROVISION_CACHE_FILE: &str = ".provision_cache.tar.gz";
const BASE_CACHE_FILE: &str = ".base_cache.tar.gz";

/// Check if a cached provisioned rootfs exists for the given environments.
pub fn has_cached_provision(environments: &[String], shell: &str) -> Option<PathBuf> {
    let path = paths::provision_cache_path(environments, shell);
    if path.exists() { Some(path) } else { None }
}

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

pub fn build_provision_cache_script() -> String {
    build_full_rootfs_cache_script(PROVISION_CACHE_FILE)
}

pub fn build_base_cache_script() -> String {
    build_full_rootfs_cache_script(BASE_CACHE_FILE)
}

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

// --- Layered cache support ---

/// Compute a short hash of a provisioning script fragment for cache keying.
/// Returns the first 12 hex characters of SHA256(script_text).
pub fn layer_version_hash(script_text: &str) -> String {
    let mut hasher = crate::sha256::Sha256::new();
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
mkdir -p /var/tmp
find / -xdev -newer /tmp/.layer_marker \
    ! -path '/proc/*' ! -path '/sys/*' ! -path '/dev/*' ! -path '/tmp/*' \
    ! -path '/var/tmp/*' ! -path '/var/cache/apt/archives/*' ! -path '/workspace/*' ! -path '/run/*' \
    -print0 2>/dev/null | \
    tar czf /var/tmp/.layer_cache.tar.gz --null -T - 2>/dev/null || true
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
    use std::collections::BTreeMap;

    fn registry() -> PluginRegistry {
        PluginRegistry::load().unwrap()
    }

    fn pv() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

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
        let script = build_base_layer_script(&SandboxShell::bash());
        assert!(script.contains("apt-get update"));
        assert!(script.contains("Creating sandbox user"));
        assert!(script.contains("groupadd"));
        assert!(script.contains("/bin/bash"));
    }

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

    #[test]
    fn env_layer_script_rust() {
        let r = registry();
        let script = build_env_layer_script("rust", &r, &pv()).unwrap();
        assert!(script.contains("Installing Rust"));
        assert!(script.contains("cp -rp /root/.cargo /home/sandbox/.cargo"));
        assert!(script.contains(".layer_marker"));
        assert!(script.contains(".layer_cache.tar.gz"));
    }

    #[test]
    fn env_layer_script_nodejs() {
        let r = registry();
        let script = build_env_layer_script("nodejs", &r, &pv()).unwrap();
        assert!(script.contains("Installing Node.js"));
        assert!(script.contains(".layer_marker"));
    }

    #[test]
    fn env_layer_script_unknown_returns_none() {
        let r = registry();
        assert!(build_env_layer_script("unknown", &r, &pv()).is_none());
    }

    #[test]
    fn env_layer_script_neovim() {
        let r = registry();
        let script = build_env_layer_script("neovim", &r, &pv()).unwrap();
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
        let status = check_layer_cache(&envs, &SandboxShell::bash(), &r, &pv());
        let total = status.cached.len() + status.uncached.len();
        assert_eq!(total, 2);
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
        // The tarball must live on a persistent rootfs path (/var/tmp), not the
        // tmpfs /tmp where it would vanish when the namespace exits.
        assert!(s.contains("tar czf /var/tmp/.provision_cache.partial"));
        // Fail-fast on any error so a partial tarball doesn't get renamed.
        assert!(s.contains("set -eo pipefail"));
    }

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
}
