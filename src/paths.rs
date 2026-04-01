use std::path::{Path, PathBuf};

pub fn isola_home() -> PathBuf {
    let home = std::env::var("HOME")
        .expect("HOME environment variable not set — cannot determine config directory");
    PathBuf::from(home).join(".isola")
}

pub fn sandboxes_dir() -> PathBuf {
    isola_home().join("sandboxes")
}

pub fn sandbox_dir(name: &str) -> PathBuf {
    sandboxes_dir().join(name)
}

pub fn rootfs_dir(name: &str) -> PathBuf {
    sandbox_dir(name).join("rootfs")
}

pub fn config_path(name: &str) -> PathBuf {
    sandbox_dir(name).join("config.json")
}

pub fn cache_dir() -> PathBuf {
    isola_home().join("cache")
}

/// Path to the project-local config: `<dir>/.isola/config.yaml`
pub fn local_config_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".isola").join("config.yaml")
}

pub fn layers_cache_dir() -> PathBuf {
    cache_dir().join("layers")
}

/// Path for a single layer cache file.
/// - Base layer: `layers/base-{hash}-{shell}.tar.gz`
/// - Env layer: `layers/env-{name}-{hash}.tar.gz`
pub fn layer_cache_path(layer_name: &str, version_hash: &str, shell: &str) -> PathBuf {
    let filename = if layer_name == "base" {
        format!("base-{version_hash}-{shell}.tar.gz")
    } else {
        format!("env-{layer_name}-{version_hash}.tar.gz")
    };
    layers_cache_dir().join(filename)
}

/// Legacy: cache path for a monolithic provisioned rootfs tarball.
pub fn provision_cache_path(environments: &[String], shell: &str) -> PathBuf {
    let mut parts: Vec<String> = environments.to_vec();
    if shell != "bash" {
        parts.push(format!("shell-{shell}"));
    }
    parts.sort();
    let key = if parts.is_empty() {
        "base".to_string()
    } else {
        parts.join("+")
    };
    cache_dir().join(format!("provisioned-{key}.tar.gz"))
}

/// User-global plugins directory: `~/.isola/plugins/`
pub fn user_plugins_dir() -> PathBuf {
    isola_home().join("plugins")
}

/// Project-local plugins directory: `<dir>/.isola/plugins/`
pub fn project_plugins_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(".isola").join("plugins")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isola_home_is_under_home() {
        let home = std::env::var("HOME").unwrap();
        let dir = isola_home();
        assert_eq!(dir, PathBuf::from(home).join(".isola"));
    }

    #[test]
    fn sandboxes_dir_is_under_isola_home() {
        assert_eq!(sandboxes_dir(), isola_home().join("sandboxes"));
    }

    #[test]
    fn sandbox_dir_uses_name() {
        let dir = sandbox_dir("test-sb");
        assert!(dir.ends_with("sandboxes/test-sb"));
    }

    #[test]
    fn rootfs_dir_uses_name() {
        let dir = rootfs_dir("test-sb");
        assert!(dir.ends_with("sandboxes/test-sb/rootfs"));
    }

    #[test]
    fn config_path_uses_name() {
        let path = config_path("test-sb");
        assert!(path.ends_with("sandboxes/test-sb/config.json"));
    }

    #[test]
    fn cache_dir_is_under_isola_home() {
        assert_eq!(cache_dir(), isola_home().join("cache"));
    }

    #[test]
    fn layers_cache_dir_is_under_cache() {
        assert_eq!(layers_cache_dir(), cache_dir().join("layers"));
    }

    #[test]
    fn layer_cache_path_base() {
        let path = layer_cache_path("base", "abc123", "bash");
        assert!(path.ends_with("layers/base-abc123-bash.tar.gz"));
    }

    #[test]
    fn layer_cache_path_env() {
        let path = layer_cache_path("rust", "def456", "bash");
        assert!(path.ends_with("layers/env-rust-def456.tar.gz"));
    }

    #[test]
    fn layer_cache_path_neovim() {
        let path = layer_cache_path("neovim", "789abc", "bash");
        assert!(path.ends_with("layers/env-neovim-789abc.tar.gz"));
    }

    #[test]
    fn local_config_path_is_correct() {
        let path = local_config_path(std::path::Path::new("/tmp/myproject"));
        assert_eq!(path, PathBuf::from("/tmp/myproject/.isola/config.yaml"));
    }

    #[test]
    fn provision_cache_path_empty_envs() {
        let path = provision_cache_path(&[], "bash");
        assert!(path.ends_with("cache/provisioned-base.tar.gz"));
    }

    #[test]
    fn provision_cache_path_sorts_envs() {
        let envs = vec!["rust".to_string(), "go".to_string(), "nodejs".to_string()];
        let path = provision_cache_path(&envs, "bash");
        assert!(path.ends_with("cache/provisioned-go+nodejs+rust.tar.gz"));
    }

    #[test]
    fn provision_cache_path_same_for_different_order() {
        let a = vec!["rust".to_string(), "nodejs".to_string()];
        let b = vec!["nodejs".to_string(), "rust".to_string()];
        assert_eq!(
            provision_cache_path(&a, "bash"),
            provision_cache_path(&b, "bash")
        );
    }

    #[test]
    fn provision_cache_path_includes_shell() {
        let envs = vec!["rust".to_string()];
        let bash_path = provision_cache_path(&envs, "bash");
        let fish_path = provision_cache_path(&envs, "fish");
        assert_ne!(bash_path, fish_path);
        assert!(fish_path.ends_with("cache/provisioned-rust+shell-fish.tar.gz"));
    }

    #[test]
    fn user_plugins_dir_is_under_isola_home() {
        assert_eq!(user_plugins_dir(), isola_home().join("plugins"));
    }

    #[test]
    fn project_plugins_dir_is_correct() {
        let path = project_plugins_dir(std::path::Path::new("/tmp/myproject"));
        assert_eq!(path, PathBuf::from("/tmp/myproject/.isola/plugins"));
    }
}
