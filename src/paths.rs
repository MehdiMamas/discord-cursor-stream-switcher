//! Where the app keeps its files. Everything stays in the user's own profile folders, except
//! what the elevated driver installer writes under %ProgramData%.

use std::path::PathBuf;

pub const APP_DIR: &str = "CursorStreamSwitcher";

fn env_dir(var: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// %APPDATA%\CursorStreamSwitcher (roams with the user profile).
pub fn config_file() -> PathBuf {
    env_dir("APPDATA").join(APP_DIR).join("config.toml")
}

/// %LOCALAPPDATA%\CursorStreamSwitcher
pub fn data_dir() -> PathBuf {
    env_dir("LOCALAPPDATA").join(APP_DIR)
}

pub fn log_file() -> PathBuf {
    data_dir().join("app.log")
}

/// %ProgramData%\CursorStreamSwitcher, written only by the elevated driver commands.
pub fn machine_dir() -> PathBuf {
    env_dir("ProgramData").join(APP_DIR)
}
