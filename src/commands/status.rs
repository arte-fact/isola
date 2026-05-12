use crate::error::IsolaError;
use crate::paths;
use crate::sandbox::config::SandboxConfig;

pub fn run(name: &str) -> Result<(), IsolaError> {
    let sandbox_dir = paths::sandbox_dir(name);
    if !sandbox_dir.exists() {
        return Err(IsolaError::SandboxNotFound(name.to_string()));
    }

    let config = SandboxConfig::load(name)?;
    let rootfs = paths::rootfs_dir(name);

    let rootfs_healthy = rootfs.join("bin").is_dir() && rootfs.join("etc").is_dir();
    let disk_usage = dir_size(&sandbox_dir);

    println!("Sandbox: {}", config.name);
    println!(
        "Created: {}",
        config.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!(
        "Environments: {}",
        if config.environments.is_empty() {
            "all".to_string()
        } else {
            config.environments.join(", ")
        }
    );
    println!(
        "Workspace: {}",
        config
            .workspace
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "Workspace exists: {}",
        config
            .workspace
            .as_ref()
            .map(|p| p.exists())
            .unwrap_or(false)
    );
    println!("Rootfs: {}", if rootfs_healthy { "ok" } else { "damaged" });
    println!("Disk usage: {}", format_size(disk_usage));
    println!("Rootfs URL: {}", config.rootfs_url);

    Ok(())
}

pub fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            // Use symlink_metadata to avoid following symlinks (prevents
            // double-counting and reading outside the sandbox directory).
            let meta = match entry.path().symlink_metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                total += dir_size(&entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(1048576), "1.0 MB");
    }

    #[test]
    fn format_size_gb() {
        assert_eq!(format_size(1073741824), "1.0 GB");
    }
}
