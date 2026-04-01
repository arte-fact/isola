use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::IsolaError;
use crate::paths;

/// Deserialized from plugin.yaml
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PluginManifest {
    pub name: String,
    pub description: String,
    pub version: String,
    pub provision: PluginProvision,
    #[serde(default)]
    pub paths: PluginPaths,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginProvision {
    pub script: String,
    #[serde(default)]
    pub verify: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PluginPaths {
    #[serde(default)]
    pub bin: Vec<String>,
    #[serde(default)]
    pub copy: Vec<CopyEntry>,
    /// Files/directories to copy from host $HOME into the sandbox rootfs
    /// before provisioning. `from` is relative to $HOME, `to` is relative
    /// to /home/sandbox/ inside the rootfs.
    #[serde(default)]
    pub host_copy: Vec<CopyEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CopyEntry {
    pub from: String,
    pub to: String,
}

/// A fully resolved plugin with manifest and script content.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Plugin {
    pub manifest: PluginManifest,
    pub install_script: String,
    pub source: PluginSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginSource {
    Project,
    User,
    Bundled,
}

struct BundledPlugin {
    manifest_yaml: &'static str,
    install_script: &'static str,
}

const BUNDLED_PLUGINS: &[BundledPlugin] = &[
    BundledPlugin {
        manifest_yaml: include_str!("../plugins/rust/plugin.yaml"),
        install_script: include_str!("../plugins/rust/install.sh"),
    },
    BundledPlugin {
        manifest_yaml: include_str!("../plugins/nodejs/plugin.yaml"),
        install_script: include_str!("../plugins/nodejs/install.sh"),
    },
    BundledPlugin {
        manifest_yaml: include_str!("../plugins/python-uv/plugin.yaml"),
        install_script: include_str!("../plugins/python-uv/install.sh"),
    },
    BundledPlugin {
        manifest_yaml: include_str!("../plugins/go/plugin.yaml"),
        install_script: include_str!("../plugins/go/install.sh"),
    },
    BundledPlugin {
        manifest_yaml: include_str!("../plugins/neovim/plugin.yaml"),
        install_script: include_str!("../plugins/neovim/install.sh"),
    },
    BundledPlugin {
        manifest_yaml: include_str!("../plugins/claude-unchained/plugin.yaml"),
        install_script: include_str!("../plugins/claude-unchained/install.sh"),
    },
];

/// In-memory collection of all available plugins.
pub struct PluginRegistry {
    plugins: Vec<Plugin>,
}

impl PluginRegistry {
    /// Load plugins from all sources: project > user > bundled.
    pub fn load_for_project(project_dir: Option<&Path>) -> Result<Self, IsolaError> {
        let mut by_name: HashMap<String, Plugin> = HashMap::new();

        // Lowest priority: bundled defaults
        for bp in BUNDLED_PLUGINS {
            match serde_yaml::from_str::<PluginManifest>(bp.manifest_yaml) {
                Ok(manifest) => {
                    let name = manifest.name.clone();
                    by_name.insert(
                        name,
                        Plugin {
                            manifest,
                            install_script: bp.install_script.to_string(),
                            source: PluginSource::Bundled,
                        },
                    );
                }
                Err(e) => {
                    eprintln!("warning: failed to parse bundled plugin: {e}");
                }
            }
        }

        // Medium priority: user plugins (~/.isola/plugins/)
        let user_dir = paths::user_plugins_dir();
        for plugin in load_plugins_from_dir(&user_dir, PluginSource::User) {
            by_name.insert(plugin.manifest.name.clone(), plugin);
        }

        // Highest priority: project plugins (.isola/plugins/)
        if let Some(dir) = project_dir {
            let project_dir = paths::project_plugins_dir(dir);
            for plugin in load_plugins_from_dir(&project_dir, PluginSource::Project) {
                by_name.insert(plugin.manifest.name.clone(), plugin);
            }
        }

        let mut plugins: Vec<Plugin> = by_name.into_values().collect();
        plugins.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));

        Ok(Self { plugins })
    }

    /// Load plugins without a project directory (bundled + user only).
    pub fn load() -> Result<Self, IsolaError> {
        Self::load_for_project(None)
    }

    /// Get a plugin by name.
    pub fn get(&self, name: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.manifest.name == name)
    }

    /// List all available plugin names.
    pub fn available_names(&self) -> Vec<&str> {
        self.plugins
            .iter()
            .map(|p| p.manifest.name.as_str())
            .collect()
    }

    /// List all available plugins.
    pub fn available_plugins(&self) -> &[Plugin] {
        &self.plugins
    }

    /// Validate that all requested environment names have plugins.
    #[allow(dead_code)]
    pub fn validate_environments(&self, envs: &[String]) -> Result<(), IsolaError> {
        for env in envs {
            if self.get(env).is_none() {
                return Err(IsolaError::PluginError(format!(
                    "unknown environment '{env}': no plugin found"
                )));
            }
        }
        Ok(())
    }
}

/// Load plugins from a filesystem directory.
/// Each subdirectory should contain a plugin.yaml and referenced script files.
fn load_plugins_from_dir(dir: &Path, source: PluginSource) -> Vec<Plugin> {
    let mut plugins = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return plugins, // directory doesn't exist, that's fine
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("plugin.yaml");
        let manifest_str = match std::fs::read_to_string(&manifest_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let manifest: PluginManifest = match serde_yaml::from_str(&manifest_str) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "warning: skipping plugin in {}: {e}",
                    manifest_path.display()
                );
                continue;
            }
        };

        let script_path = path.join(&manifest.provision.script);
        let install_script = match std::fs::read_to_string(&script_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "warning: skipping plugin '{}': cannot read {}: {e}",
                    manifest.name,
                    script_path.display()
                );
                continue;
            }
        };

        plugins.push(Plugin {
            manifest,
            install_script,
            source: source.clone(),
        });
    }

    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_plugins_all_parse() {
        let registry = PluginRegistry::load().unwrap();
        let names = registry.available_names();
        assert!(names.contains(&"rust"));
        assert!(names.contains(&"nodejs"));
        assert!(names.contains(&"python-uv"));
        assert!(names.contains(&"go"));
        assert!(names.contains(&"neovim"));
    }

    #[test]
    fn bundled_plugins_have_install_scripts() {
        let registry = PluginRegistry::load().unwrap();
        for plugin in registry.available_plugins() {
            assert!(
                !plugin.install_script.is_empty(),
                "plugin '{}' has empty install script",
                plugin.manifest.name
            );
        }
    }

    #[test]
    fn bundled_plugins_are_bundled_source() {
        let registry = PluginRegistry::load().unwrap();
        for plugin in registry.available_plugins() {
            assert_eq!(plugin.source, PluginSource::Bundled);
        }
    }

    #[test]
    fn get_existing_plugin() {
        let registry = PluginRegistry::load().unwrap();
        let rust = registry.get("rust").unwrap();
        assert_eq!(rust.manifest.name, "rust");
        assert!(rust.install_script.contains("rustup"));
    }

    #[test]
    fn get_nonexistent_plugin() {
        let registry = PluginRegistry::load().unwrap();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn validate_known_environments() {
        let registry = PluginRegistry::load().unwrap();
        let envs = vec!["rust".into(), "go".into()];
        assert!(registry.validate_environments(&envs).is_ok());
    }

    #[test]
    fn validate_unknown_environment() {
        let registry = PluginRegistry::load().unwrap();
        let envs = vec!["rust".into(), "haskell".into()];
        let err = registry.validate_environments(&envs).unwrap_err();
        assert!(err.to_string().contains("haskell"));
    }

    #[test]
    fn plugin_manifest_has_paths() {
        let registry = PluginRegistry::load().unwrap();
        let rust = registry.get("rust").unwrap();
        assert!(!rust.manifest.paths.bin.is_empty());
        assert!(!rust.manifest.paths.copy.is_empty());
    }

    #[test]
    fn plugin_manifest_verify_command() {
        let registry = PluginRegistry::load().unwrap();
        let rust = registry.get("rust").unwrap();
        assert!(rust.manifest.provision.verify.is_some());
        assert!(
            rust.manifest
                .provision
                .verify
                .as_ref()
                .unwrap()
                .contains("rustc")
        );
    }

    #[test]
    fn filesystem_plugin_overrides_bundled() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("rust");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            "name: rust\ndescription: Custom Rust\nversion: '2.0.0'\nprovision:\n  script: install.sh\n",
        )
        .unwrap();
        std::fs::write(plugin_dir.join("install.sh"), "echo custom rust").unwrap();

        let plugins = load_plugins_from_dir(dir.path(), PluginSource::User);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].manifest.description, "Custom Rust");
        assert_eq!(plugins[0].manifest.version, "2.0.0");
    }

    #[test]
    fn load_from_nonexistent_dir() {
        let plugins = load_plugins_from_dir(Path::new("/nonexistent/path"), PluginSource::User);
        assert!(plugins.is_empty());
    }
}
