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

#[cfg(target_os = "linux")]
pub fn rootfs_dir(name: &str) -> PathBuf {
    sandbox_dir(name).join("rootfs")
}

pub fn config_path(name: &str) -> PathBuf {
    sandbox_dir(name).join("config.json")
}

#[cfg(target_os = "linux")]
pub fn cache_dir() -> PathBuf {
    isola_home().join("cache")
}

pub fn session_dir() -> PathBuf {
    isola_home().join("session")
}

pub fn session_credentials() -> PathBuf {
    session_dir().join(".credentials.json")
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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
}
