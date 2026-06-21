use crate::commands::status::{dir_size, format_size};
use isola_core::error::IsolaError;
use isola_core::paths;
use isola_core::sandbox::backend;
use isola_core::sandbox::config::SandboxConfig;

pub fn run() -> Result<(), IsolaError> {
    let sandboxes_dir = paths::sandboxes_dir();
    if !sandboxes_dir.exists() {
        eprintln!("No sandboxes found");
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&sandboxes_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    if entries.is_empty() {
        eprintln!("No sandboxes found");
        return Ok(());
    }

    entries.sort_by_key(|e| e.file_name());

    let b = backend::create_backend();
    let backend_name = b.backend_name();

    println!(
        "{:<20} {:<16} {:<24} {:<20} {:<10} WORKSPACE",
        "NAME", "BACKEND", "CREATED", "ENVIRONMENTS", "SIZE"
    );
    println!("{}", "-".repeat(116));

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let size = format_size(dir_size(&entry.path()));
        match SandboxConfig::load(&name) {
            Ok(config) => {
                let workspace = config
                    .workspace
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "-".to_string());
                let envs = if config.environments.is_empty() {
                    "all".to_string()
                } else {
                    config.environments.join(",")
                };
                println!(
                    "{:<20} {:<16} {:<24} {:<20} {:<10} {}",
                    config.name,
                    backend_name,
                    config.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
                    envs,
                    size,
                    workspace
                );
            }
            Err(_) => {
                println!(
                    "{:<20} {:<16} {:<24} {:<20} {:<10} ?",
                    name, "?", "?", "?", size
                );
            }
        }
    }

    Ok(())
}
