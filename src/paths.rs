use std::path::PathBuf;

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

pub fn session_dir() -> PathBuf {
    isola_home().join("session")
}

pub fn session_credentials() -> PathBuf {
    session_dir().join(".credentials.json")
}

pub fn host_claude_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .expect("HOME environment variable not set — cannot determine config directory");
    PathBuf::from(home).join(".claude")
}

pub fn host_claude_credentials() -> PathBuf {
    host_claude_dir().join(".credentials.json")
}

pub fn host_claude_settings() -> PathBuf {
    host_claude_dir().join("settings.json")
}

/// Cache path for a provisioned rootfs tarball, keyed by sorted environment names,
/// shell choice, and extras (e.g. neovim).
pub fn provision_cache_path(environments: &[String], shell: &str, install_neovim: bool) -> PathBuf {
    let mut parts: Vec<String> = environments.to_vec();
    if shell != "bash" {
        parts.push(format!("shell-{shell}"));
    }
    if install_neovim {
        parts.push("neovim".to_string());
    }
    parts.sort();
    let key = if parts.is_empty() {
        "base".to_string()
    } else {
        parts.join("+")
    };
    cache_dir().join(format!("provisioned-{key}.tar.gz"))
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
    fn session_dir_is_under_isola_home() {
        assert_eq!(session_dir(), isola_home().join("session"));
    }

    #[test]
    fn session_credentials_is_under_session_dir() {
        assert_eq!(
            session_credentials(),
            session_dir().join(".credentials.json")
        );
    }

    #[test]
    fn host_claude_credentials_path() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            host_claude_credentials(),
            PathBuf::from(home)
                .join(".claude")
                .join(".credentials.json")
        );
    }

    #[test]
    fn host_claude_settings_path() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            host_claude_settings(),
            PathBuf::from(home).join(".claude").join("settings.json")
        );
    }

    #[test]
    fn provision_cache_path_empty_envs() {
        let path = provision_cache_path(&[], "bash", false);
        assert!(path.ends_with("cache/provisioned-base.tar.gz"));
    }

    #[test]
    fn provision_cache_path_sorts_envs() {
        let envs = vec!["rust".to_string(), "go".to_string(), "nodejs".to_string()];
        let path = provision_cache_path(&envs, "bash", false);
        assert!(path.ends_with("cache/provisioned-go+nodejs+rust.tar.gz"));
    }

    #[test]
    fn provision_cache_path_same_for_different_order() {
        let a = vec!["rust".to_string(), "nodejs".to_string()];
        let b = vec!["nodejs".to_string(), "rust".to_string()];
        assert_eq!(
            provision_cache_path(&a, "bash", false),
            provision_cache_path(&b, "bash", false)
        );
    }

    #[test]
    fn provision_cache_path_includes_shell() {
        let envs = vec!["rust".to_string()];
        let bash_path = provision_cache_path(&envs, "bash", false);
        let fish_path = provision_cache_path(&envs, "fish", false);
        assert_ne!(bash_path, fish_path);
        assert!(fish_path.ends_with("cache/provisioned-rust+shell-fish.tar.gz"));
    }

    #[test]
    fn provision_cache_path_includes_neovim() {
        let envs = vec!["rust".to_string()];
        let without = provision_cache_path(&envs, "bash", false);
        let with = provision_cache_path(&envs, "bash", true);
        assert_ne!(without, with);
        assert!(with.ends_with("cache/provisioned-neovim+rust.tar.gz"));
    }
}
