use isola_core::error::IsolaError;
use isola_core::plugin::{PluginLayer, PluginRegistry, PluginSource};

/// `isola plugins` — list every available plugin by layer, with its source.
///
/// Resolution is project > user > bundled (by name), so a plugin shown as
/// `user` or `project` overrides a bundled one of the same name.
pub fn list() -> Result<(), IsolaError> {
    let project_dir = std::env::current_dir().ok();
    let registry = PluginRegistry::load_for_project(project_dir.as_deref())?;

    println!("Available plugins  (source overrides: project > user > bundled)\n");

    for (layer, title) in [
        (PluginLayer::Project, "Project tooling (install software)"),
        (PluginLayer::User, "User setup (import from host)"),
        (PluginLayer::Shell, "Shells"),
    ] {
        let plugins = registry.plugins_for_layer(layer);
        if plugins.is_empty() {
            continue;
        }
        println!("{title}:");
        for p in plugins {
            let src = match p.source {
                PluginSource::Bundled => "bundled",
                PluginSource::User => "user",
                PluginSource::Project => "project",
            };
            println!(
                "  {:<14} v{:<7} [{:<7}] {}",
                p.manifest.name, p.manifest.version, src, p.manifest.description
            );
        }
        println!();
    }

    println!(
        "Drop a plugin in ~/.isola/plugins/<name>/ (user) or <project>/.isola/plugins/<name>/ (project)."
    );
    Ok(())
}
