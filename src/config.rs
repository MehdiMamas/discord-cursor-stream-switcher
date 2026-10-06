//! User settings, stored as TOML in %APPDATA%\CursorStreamSwitcher\config.toml.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// How long (ms) the cursor must stay on another monitor before the stream switches to it.
    pub switch_delay_ms: u64,
    /// Draw the mouse cursor into the stream.
    pub show_cursor: bool,
    /// Frame cap for the stream screen (10-240). Discord streams at most 60.
    pub max_fps: u32,
    /// Stop the mouse from wandering onto the virtual stream display.
    pub keep_mouse_off_stream_display: bool,
    /// Monitors that are never shown. The stream shows a "hidden" card while the cursor is on them.
    pub hidden_monitors: Vec<String>,
    /// Pause/resume the stream (shows a "paused" card). Empty string disables the hotkey.
    pub hotkey_pause: String,
    /// Lock the stream to the current monitor. Empty string disables the hotkey.
    pub hotkey_lock: String,
    /// Check GitHub Releases for a new version once a day. Nothing else is ever sent anywhere.
    pub check_for_updates: bool,
    /// "auto" finds the Virtual Display Driver screen. Or a display name such as "\\.\DISPLAY3".
    pub stream_display: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            switch_delay_ms: 150,
            show_cursor: true,
            max_fps: 60,
            keep_mouse_off_stream_display: true,
            hidden_monitors: Vec::new(),
            hotkey_pause: "Ctrl+Alt+P".into(),
            hotkey_lock: "Ctrl+Alt+L".into(),
            check_for_updates: true,
            stream_display: "auto".into(),
        }
    }
}

const HEADER: &str = "\
# Cursor Stream Switcher settings. Use the tray menu > Reload settings after editing.
#
# switch_delay_ms               ms the cursor must stay on a monitor before the stream switches
# show_cursor                   draw the mouse cursor into the stream
# max_fps                       frame cap for the stream screen, 10-240 (Discord streams at most 60)
# keep_mouse_off_stream_display stop the mouse from entering the virtual stream screen
# hidden_monitors               monitor IDs never shown (easier: tray menu > Hide from stream)
# hotkey_pause / hotkey_lock    e.g. \"Ctrl+Alt+P\", \"Ctrl+Shift+F9\"; \"\" disables
# check_for_updates             daily check of this app's GitHub Releases; nothing else is sent
# stream_display                \"auto\", or a display name in single quotes like '\\\\.\\DISPLAY3'

";

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> String {
        let body = toml::to_string_pretty(self).expect("config always serializes");
        format!("{HEADER}{body}")
    }

    /// Loads the config, writing the defaults if the file doesn't exist yet. A broken file is
    /// left untouched (so the user's edits aren't lost) and the defaults are used.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::parse(&text) {
                Ok(cfg) => (cfg, None),
                Err(e) => (Self::default(), Some(format!("Settings file has an error, using defaults: {e}"))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Self::default();
                let err = cfg.save(path).err();
                (cfg, err)
            }
            Err(e) => (Self::default(), Some(format!("Can't read settings file: {e}"))),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn is_hidden(&self, monitor_id: &str) -> bool {
        self.hidden_monitors.iter().any(|m| m.eq_ignore_ascii_case(monitor_id))
    }

    pub fn set_hidden(&mut self, monitor_id: &str, hidden: bool) {
        self.hidden_monitors.retain(|m| !m.eq_ignore_ascii_case(monitor_id));
        if hidden {
            self.hidden_monitors.push(monitor_id.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let cfg = Config::default();
        assert_eq!(Config::parse(&cfg.to_toml()).unwrap(), cfg);
    }

    #[test]
    fn missing_keys_use_defaults() {
        let cfg = Config::parse("switch_delay_ms = 0\n").unwrap();
        assert_eq!(cfg.switch_delay_ms, 0);
        assert!(cfg.show_cursor);
    }

    #[test]
    fn unknown_keys_and_bad_types_are_errors() {
        assert!(Config::parse("swich_delay_ms = 5\n").is_err());
        assert!(Config::parse("show_cursor = \"yes\"\n").is_err());
    }

    #[test]
    fn hidden_monitors_toggle_case_insensitively() {
        let mut cfg = Config::default();
        cfg.set_hidden(r"\\?\DISPLAY#ABC", true);
        assert!(cfg.is_hidden(r"\\?\display#abc"));
        cfg.set_hidden(r"\\?\display#abc", true);
        assert_eq!(cfg.hidden_monitors.len(), 1);
        cfg.set_hidden(r"\\?\DISPLAY#ABC", false);
        assert!(cfg.hidden_monitors.is_empty());
    }

    #[test]
    fn load_creates_file_and_keeps_broken_file() {
        let dir = std::env::temp_dir().join(format!("css-config-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let (cfg, err) = Config::load(&path);
        assert!(err.is_none());
        assert_eq!(cfg, Config::default());
        assert!(path.exists());

        std::fs::write(&path, "show_cursor = 3").unwrap();
        let (cfg, err) = Config::load(&path);
        assert!(err.is_some());
        assert_eq!(cfg, Config::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "show_cursor = 3");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
