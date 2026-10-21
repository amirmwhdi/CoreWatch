//! Persistent user settings stored in ~/.config/corewatch/settings.ini.

use crate::config::DEFAULT_SAMPLE_INTERVAL;
use gtk::glib;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    System = 0,
    Light = 1,
    Dark = 2,
}

impl ThemeMode {
    pub fn from_index(index: u32) -> Self {
        match index {
            1 => Self::Light,
            2 => Self::Dark,
            _ => Self::System,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpdateSpeed {
    HalfSecond = 0, // 500ms (0.5s)
    #[default]
    Normal = 1, // 1000ms (1s)
    Slow = 2,       // 2000ms (2s)
}

impl UpdateSpeed {
    pub fn from_index(index: u32) -> Self {
        match index {
            0 => Self::HalfSecond,
            2 => Self::Slow,
            _ => Self::Normal,
        }
    }

    pub fn as_duration(&self) -> Duration {
        match self {
            Self::HalfSecond => Duration::from_millis(500),
            Self::Normal => DEFAULT_SAMPLE_INTERVAL,
            Self::Slow => Duration::from_millis(2000),
        }
    }

    pub fn as_millis(&self) -> u64 {
        self.as_duration().as_millis() as u64
    }

    pub fn from_millis(ms: u64) -> Self {
        match ms {
            0..=750 => Self::HalfSecond,
            751..=1500 => Self::Normal,
            _ => Self::Slow,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AppSettings {
    pub theme: ThemeMode,
    pub speed: UpdateSpeed,
}

impl AppSettings {
    fn file_path() -> PathBuf {
        glib::user_config_dir()
            .join("corewatch")
            .join("settings.ini")
    }

    pub fn load() -> Self {
        let path = Self::file_path();
        let key_file = glib::KeyFile::new();
        if key_file
            .load_from_file(&path, glib::KeyFileFlags::NONE)
            .is_ok()
        {
            let theme = key_file
                .string("General", "theme")
                .map(|s| ThemeMode::from_str(s.as_str()))
                .unwrap_or_default();
            let speed = key_file
                .int64("General", "update_interval_ms")
                .map(|ms| UpdateSpeed::from_millis(ms as u64))
                .unwrap_or_default();
            Self { theme, speed }
        } else {
            Self::default()
        }
    }

    pub fn save(&self) {
        let path = Self::file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let key_file = glib::KeyFile::new();
        key_file.set_string("General", "theme", self.theme.as_str());
        key_file.set_int64(
            "General",
            "update_interval_ms",
            self.speed.as_millis() as i64,
        );
        let _ = key_file.save_to_file(&path);
    }

    pub fn apply_theme(&self) {
        let style = adw::StyleManager::default();
        match self.theme {
            ThemeMode::System => style.set_color_scheme(adw::ColorScheme::Default),
            ThemeMode::Light => style.set_color_scheme(adw::ColorScheme::ForceLight),
            ThemeMode::Dark => style.set_color_scheme(adw::ColorScheme::ForceDark),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_mode_roundtrip() {
        assert_eq!(ThemeMode::from_index(0), ThemeMode::System);
        assert_eq!(ThemeMode::from_index(1), ThemeMode::Light);
        assert_eq!(ThemeMode::from_index(2), ThemeMode::Dark);

        assert_eq!(ThemeMode::from_str("light"), ThemeMode::Light);
        assert_eq!(ThemeMode::from_str("dark"), ThemeMode::Dark);
        assert_eq!(ThemeMode::from_str("other"), ThemeMode::System);
    }

    #[test]
    fn update_speed_roundtrip() {
        assert_eq!(UpdateSpeed::from_index(0), UpdateSpeed::HalfSecond);
        assert_eq!(UpdateSpeed::from_index(1), UpdateSpeed::Normal);
        assert_eq!(UpdateSpeed::from_index(2), UpdateSpeed::Slow);

        assert_eq!(
            UpdateSpeed::HalfSecond.as_duration(),
            Duration::from_millis(500)
        );
        assert_eq!(
            UpdateSpeed::Normal.as_duration(),
            Duration::from_millis(1000)
        );
        assert_eq!(UpdateSpeed::Slow.as_duration(), Duration::from_millis(2000));
    }
}
