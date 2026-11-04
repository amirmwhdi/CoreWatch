//! Persistent user settings stored in ~/.config/corewatch/settings.ini.

use crate::config::DEFAULT_SAMPLE_INTERVAL;
use crate::format::{NetworkUnits, SizeUnits, TemperatureUnit, Units};
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

/// How far back graphs reach, in minutes, as Preferences offers it.
pub const HISTORY_MINUTES: [u32; 4] = [1, 5, 15, 60];

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

    /// Points a graph keeps: `minutes` of samples at this speed.
    pub fn history_points(&self, minutes: u32) -> usize {
        let history = Duration::from_secs(u64::from(minutes) * 60);
        let points = history.as_millis() / self.as_duration().as_millis().max(1);
        usize::try_from(points).unwrap_or(usize::MAX).max(2)
    }

    pub fn from_millis(ms: u64) -> Self {
        match ms {
            0..=750 => Self::HalfSecond,
            751..=1500 => Self::Normal,
            _ => Self::Slow,
        }
    }
}

/// The page shown when Corewatch starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartPage {
    /// The page that was open when Corewatch was closed.
    Last,
    /// Always this page, by id (`"overview"`, `"cpu"`, …).
    Page(String),
}

impl Default for StartPage {
    fn default() -> Self {
        Self::Page("overview".into())
    }
}

impl StartPage {
    fn as_str(&self) -> &str {
        match self {
            Self::Last => "last",
            Self::Page(id) => id,
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "last" => Self::Last,
            "" => Self::default(),
            id => Self::Page(id.to_owned()),
        }
    }
}

/// The limits past which Corewatch sends a notification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlertSettings {
    pub enabled: bool,
    /// Processor or graphics card temperature, in °C.
    pub temperature_c: f32,
    /// Share of memory in use, 0.0..=1.0.
    pub memory_share: f32,
    /// Share of a filesystem in use, 0.0..=1.0.
    pub disk_share: f32,
    /// A process keeps a whole core busy for five minutes.
    pub busy_process: bool,
    /// Closing the window leaves Corewatch running, to keep watching.
    pub background: bool,
}

impl Default for AlertSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            temperature_c: 90.0,
            memory_share: 0.9,
            disk_share: 0.95,
            busy_process: true,
            background: false,
        }
    }
}

thread_local! {
    /// The settings as last loaded or saved, for code that reads them every
    /// tick (alerts) and must not read the file each time.
    static CACHED: std::cell::RefCell<Option<AppSettings>> = const { std::cell::RefCell::new(None) };
}

#[derive(Debug, Clone)]
pub struct AppSettings {
    pub theme: ThemeMode,
    pub speed: UpdateSpeed,
    /// Graphs scroll continuously between samples; off, they move once per
    /// sample and draw nothing in between, which saves power.
    pub smooth_graphs: bool,
    pub units: Units,
    pub start_page: StartPage,
    /// The page open when Corewatch was last closed.
    pub last_page: String,
    /// The process list groups processes by app.
    pub group_processes: bool,
    /// How far back graphs reach; one of [`HISTORY_MINUTES`].
    pub history_minutes: u32,
    pub alerts: AlertSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::default(),
            speed: UpdateSpeed::default(),
            smooth_graphs: true,
            units: Units::default(),
            start_page: StartPage::default(),
            last_page: String::new(),
            group_processes: false,
            history_minutes: 1,
            alerts: AlertSettings::default(),
        }
    }
}

impl AppSettings {
    fn file_path() -> PathBuf {
        glib::user_config_dir()
            .join("corewatch")
            .join("settings.ini")
    }

    /// The settings as last loaded or saved, without reading the file again.
    pub fn load_cached() -> Self {
        CACHED
            .with(|c| c.borrow().clone())
            .unwrap_or_else(Self::load)
    }

    pub fn load() -> Self {
        let settings = Self::read();
        CACHED.with(|c| c.replace(Some(settings.clone())));
        settings
    }

    fn read() -> Self {
        let path = Self::file_path();
        let key_file = glib::KeyFile::new();
        if key_file
            .load_from_file(&path, glib::KeyFileFlags::NONE)
            .is_ok()
        {
            let text = |key: &str| key_file.string("General", key).ok().map(String::from);
            let units = Units {
                temperature: match text("temperature").as_deref() {
                    Some("fahrenheit") => TemperatureUnit::Fahrenheit,
                    _ => TemperatureUnit::Celsius,
                },
                sizes: match text("sizes").as_deref() {
                    Some("decimal") => SizeUnits::Decimal,
                    _ => SizeUnits::Binary,
                },
                network: match text("network_speed").as_deref() {
                    Some("bits") => NetworkUnits::Bits,
                    _ => NetworkUnits::Bytes,
                },
            };
            Self {
                theme: text("theme")
                    .map(|s| ThemeMode::from_str(&s))
                    .unwrap_or_default(),
                speed: key_file
                    .int64("General", "update_interval_ms")
                    .map(|ms| UpdateSpeed::from_millis(ms as u64))
                    .unwrap_or_default(),
                smooth_graphs: key_file.boolean("General", "smooth_graphs").unwrap_or(true),
                units,
                start_page: text("start_page")
                    .map(|s| StartPage::from_str(&s))
                    .unwrap_or_default(),
                last_page: text("last_page").unwrap_or_default(),
                group_processes: key_file
                    .boolean("General", "group_processes")
                    .unwrap_or(false),
                history_minutes: key_file
                    .integer("General", "history_minutes")
                    .ok()
                    .and_then(|m| u32::try_from(m).ok())
                    .filter(|m| HISTORY_MINUTES.contains(m))
                    .unwrap_or(1),
                alerts: {
                    let default = AlertSettings::default();
                    let flag = |key: &str, d: bool| key_file.boolean("Alerts", key).unwrap_or(d);
                    let number = |key: &str, d: f32, range: std::ops::RangeInclusive<f32>| {
                        key_file
                            .double("Alerts", key)
                            .ok()
                            .map(|v| v as f32)
                            .filter(|v| range.contains(v))
                            .unwrap_or(d)
                    };
                    AlertSettings {
                        enabled: flag("enabled", default.enabled),
                        temperature_c: number("temperature_c", default.temperature_c, 40.0..=120.0),
                        memory_share: number("memory_share", default.memory_share, 0.5..=1.0),
                        disk_share: number("disk_share", default.disk_share, 0.5..=1.0),
                        busy_process: flag("busy_process", default.busy_process),
                        background: flag("background", default.background),
                    }
                },
            }
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
        key_file.set_boolean("General", "smooth_graphs", self.smooth_graphs);
        let units = self.units;
        key_file.set_string(
            "General",
            "temperature",
            match units.temperature {
                TemperatureUnit::Celsius => "celsius",
                TemperatureUnit::Fahrenheit => "fahrenheit",
            },
        );
        key_file.set_string(
            "General",
            "sizes",
            match units.sizes {
                SizeUnits::Binary => "binary",
                SizeUnits::Decimal => "decimal",
            },
        );
        key_file.set_string(
            "General",
            "network_speed",
            match units.network {
                NetworkUnits::Bytes => "bytes",
                NetworkUnits::Bits => "bits",
            },
        );
        key_file.set_string("General", "start_page", self.start_page.as_str());
        key_file.set_string("General", "last_page", &self.last_page);
        key_file.set_boolean("General", "group_processes", self.group_processes);
        key_file.set_integer(
            "General",
            "history_minutes",
            i32::try_from(self.history_minutes).unwrap_or(1),
        );
        let a = &self.alerts;
        key_file.set_boolean("Alerts", "enabled", a.enabled);
        key_file.set_double("Alerts", "temperature_c", f64::from(a.temperature_c));
        key_file.set_double("Alerts", "memory_share", f64::from(a.memory_share));
        key_file.set_double("Alerts", "disk_share", f64::from(a.disk_share));
        key_file.set_boolean("Alerts", "busy_process", a.busy_process);
        key_file.set_boolean("Alerts", "background", a.background);
        CACHED.with(|c| c.replace(Some(self.clone())));
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
    fn history_points_follow_the_interval() {
        assert_eq!(UpdateSpeed::Normal.history_points(1), 60);
        assert_eq!(UpdateSpeed::HalfSecond.history_points(1), 120);
        assert_eq!(UpdateSpeed::Slow.history_points(1), 30);
        assert_eq!(UpdateSpeed::HalfSecond.history_points(60), 7200);
    }

    #[test]
    fn start_page_roundtrip() {
        assert_eq!(StartPage::from_str("last"), StartPage::Last);
        assert_eq!(StartPage::from_str("cpu"), StartPage::Page("cpu".into()));
        assert_eq!(StartPage::from_str(""), StartPage::default());
        assert_eq!(StartPage::Last.as_str(), "last");
    }

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
