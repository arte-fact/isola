use std::collections::BTreeMap;
use std::path::PathBuf;

use isola_core::create::run_with_envs as core_run_with_envs;
use isola_core::error::IsolaError;
use isola_core::plugin::{PluginLayer, PluginRegistry};
use isola_core::sandbox::config::SandboxShell;

pub use isola_core::create::CreateRequest;

/// `isola create <name>` — build the environment list (all project plugins, or
/// the requested ones, plus auto-detected host imports), then create the sandbox.
pub fn run(
    name: &str,
    workspace: Option<PathBuf>,
    no_cache: bool,
    plugins: Vec<String>,
) -> Result<(), IsolaError> {
    let registry = PluginRegistry::load()?;
    let home = std::env::var("HOME").ok().map(PathBuf::from);

    let mut envs: Vec<String> = if plugins.is_empty() {
        registry
            .plugins_for_layer(PluginLayer::Project)
            .into_iter()
            .map(|p| p.manifest.name.clone())
            .collect()
    } else {
        registry.validate_environments(&plugins)?;
        plugins
    };

    // Auto-add user-layer plugins whose host path is detected (e.g. claude-config, ssh-keys)
    for p in registry.plugins_for_layer(PluginLayer::User) {
        if let Some(ref ad) = p.manifest.auto_detect
            && home
                .as_ref()
                .map(|h| h.join(&ad.host_path).exists())
                .unwrap_or(false)
        {
            envs.push(p.manifest.name.clone());
        }
    }

    run_with_envs(CreateRequest {
        name,
        workspace,
        environments: &envs,
        share_display: false,
        no_cache,
        shell: &SandboxShell::default(),
        registry: &registry,
        plugin_vars: &BTreeMap::new(),
    })
}

/// CLI create: offer one-time host setup if needed, attach the live progress UI,
/// then hand off to the engine. (The library's `Sandbox::create` skips both.)
pub fn run_with_envs(req: CreateRequest) -> Result<(), IsolaError> {
    #[cfg(target_os = "linux")]
    super::setup_host::ensure_userns_allowed()?;

    #[cfg(target_os = "linux")]
    {
        let progress = crate::progress::CreationProgress::new(req.name);
        core_run_with_envs(req, &progress)
    }

    #[cfg(target_os = "macos")]
    {
        core_run_with_envs(req, &isola_core::NoProgress)
    }
}
