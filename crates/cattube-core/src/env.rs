//! Where CatTube keeps its files, and small environment helpers.

use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
}

/// `~/.config/cattube`: accounts and settings.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("cattube")
}

/// `~/.cache/cattube`: pictures from the server.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("cattube")
}

/// Short-lived files (request headers, the player's control socket).
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// The player's on-screen controls and key bindings (`cattube-osd.lua`,
/// `cattube-input.conf`): `$CATTUBE_DATA`, then `../share/cattube` next to the
/// binary (packages, AppImage), then `assets/` (running from the source).
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CATTUBE_DATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(prefix) = exe.parent().and_then(|b| b.parent())
    {
        let d = prefix.join("share/cattube");
        if d.join("cattube-osd.lua").is_file() {
            return d;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
