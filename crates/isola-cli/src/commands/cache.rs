use std::path::PathBuf;

use isola_core::error::IsolaError;
use isola_core::paths;

/// `isola cache clean [--all]` — remove cached downloads.
///
/// By default clears the shared package caches (`~/.isola/cache/pkg`). With
/// `--all`, also clears the provisioned-rootfs layer caches. The apt cache
/// contains files owned by mapped subordinate UIDs (provisioning runs as
/// in-namespace root), so on Linux removal goes through a user namespace.
pub fn clean(all: bool) -> Result<(), IsolaError> {
    let mut paths: Vec<PathBuf> = vec![paths::cache_dir().join("pkg")];

    if all {
        #[cfg(target_os = "linux")]
        paths.push(paths::layers_cache_dir());
        // Legacy monolithic provision caches: provisioned-*.tar.gz
        if let Ok(entries) = std::fs::read_dir(paths::cache_dir()) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("provisioned-") && name.ends_with(".tar.gz") {
                    paths.push(entry.path());
                }
            }
        }
    }

    let existing: Vec<PathBuf> = paths.into_iter().filter(|p| p.exists()).collect();
    if existing.is_empty() {
        eprintln!("Nothing to clean.");
        return Ok(());
    }

    for p in &existing {
        eprintln!("Removing {}", p.display());
    }

    #[cfg(target_os = "linux")]
    isola_core::sandbox::linux::cleanup::remove_paths(existing)?;

    #[cfg(not(target_os = "linux"))]
    for p in &existing {
        if p.is_dir() {
            std::fs::remove_dir_all(p)?;
        } else {
            std::fs::remove_file(p)?;
        }
    }

    eprintln!("Cache cleaned.");
    Ok(())
}
