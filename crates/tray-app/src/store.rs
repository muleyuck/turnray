//! Where the settings live between launches.

use std::path::PathBuf;

use agent_core::Settings;

fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/turnray/settings"))
}

/// Defaults when there is no file yet or it can't be read.
pub fn load() -> Settings {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| Settings::parse(&text))
        .unwrap_or_default()
}

/// A failed save only costs the setting at the next launch, so it is logged, not shown.
pub fn save(settings: &Settings) {
    let Some(path) = path() else {
        tracing::warn!("HOME is unset; settings not saved");
        return;
    };
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, settings.serialize()));
    if let Err(e) = result {
        tracing::warn!("saving settings to {} failed: {e}", path.display());
    }
}
