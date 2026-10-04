//! Local window preferences, independent of MPD. Coordinates are AppKit points
//! (global bottom-left origin), never backing pixels or Slint scaled coordinates.
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::PathBuf,
    time::{Duration, Instant},
};

pub const RATIO: f64 = 420.0 / 640.0;
pub const MIN_WIDTH: f64 = 315.0;
pub const MAX_WIDTH: f64 = 630.0;
const QUIET_PERIOD: Duration = Duration::from_millis(750);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Geometry {
    fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width > 0.0
            && self.height > 0.0
    }
    pub fn canonical(self) -> Self {
        Self {
            width: 420.0,
            height: 640.0,
            y: self.y + self.height - 640.0,
            ..self
        }
    }
    /// Prefer the screen with greatest overlap; disconnected displays fall back
    /// to the main screen. Preserve exact valid geometry, otherwise fit and clamp.
    pub fn restored(self, screens: &[Self]) -> Self {
        let mut frame = if self.valid() {
            self
        } else {
            Self {
                x: 0.0,
                y: 0.0,
                width: 420.0,
                height: 640.0,
            }
        };
        frame.width = frame.width.clamp(MIN_WIDTH, MAX_WIDTH);
        frame.height = frame.width / RATIO;
        let overlap = |screen: &Self| {
            ((frame.x + frame.width).min(screen.x + screen.width) - frame.x.max(screen.x)).max(0.0)
                * ((frame.y + frame.height).min(screen.y + screen.height) - frame.y.max(screen.y))
                    .max(0.0)
        };
        let Some(mut screen) = screens.iter().find(|s| s.valid()) else {
            return frame;
        };
        let mut best = overlap(screen);
        for candidate in screens.iter().filter(|s| s.valid()) {
            let area = overlap(candidate);
            if area > best {
                screen = candidate;
                best = area;
            }
        }
        frame.width = frame
            .width
            .min(screen.width.min(screen.height * RATIO).max(MIN_WIDTH));
        frame.height = frame.width / RATIO;
        if best == 0.0 {
            frame.x = screen.x + (screen.width - frame.width) / 2.0;
            frame.y = screen.y + (screen.height - frame.height) / 2.0;
        }
        frame.x = frame.x.clamp(
            screen.x,
            (screen.x + screen.width - frame.width).max(screen.x),
        );
        // On an unusually tiny desktop keep the top (MENU/display) reachable,
        // rather than violating the minimum readable size.
        frame.y = frame.y.clamp(
            screen.y.min(screen.y + screen.height - frame.height),
            screen.y + screen.height - frame.height,
        );
        frame
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub always_on_top: bool,
    pub geometry: Option<Geometry>,
    // Do not discard future/unrelated settings when saving window preferences.
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

pub struct Store {
    pub preferences: Preferences,
    path: Option<PathBuf>,
    changed: Option<Instant>,
}
impl Store {
    pub fn load(path: Option<PathBuf>) -> Self {
        let preferences = path
            .as_ref()
            .and_then(|path| match fs::read(path) {
                Ok(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(prefs) => Some(prefs),
                    Err(err) => {
                        eprintln!("Window preferences invalid: {err}");
                        None
                    }
                },
                Err(err) if err.kind() == io::ErrorKind::NotFound => None,
                Err(err) => {
                    eprintln!("Cannot read window preferences: {err}");
                    None
                }
            })
            .unwrap_or_default();
        Self {
            preferences,
            path,
            changed: None,
        }
    }
    pub fn record(&mut self, geometry: Geometry, now: Instant) {
        if geometry.valid() && self.preferences.geometry != Some(geometry) {
            self.preferences.geometry = Some(geometry);
            self.changed = Some(now);
        }
    }
    pub fn pin(&mut self, value: bool) {
        self.preferences.always_on_top = value;
        self.changed = Some(Instant::now());
    }
    pub fn due(&self, now: Instant) -> bool {
        self.changed
            .is_some_and(|changed| now.duration_since(changed) >= QUIET_PERIOD)
    }
    pub fn flush(&mut self) -> io::Result<()> {
        if self.changed.is_none() {
            return Ok(());
        }
        let Some(path) = &self.path else {
            self.changed = None;
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let result = (|| {
            use std::io::Write;
            let mut options = fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(&self.preferences)?)?;
            file.sync_all()?;
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        // A failed write is reported once, then retried on the next change.
        self.changed = None;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screen() -> Geometry {
        Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        }
    }
    #[test]
    fn geometry_restores_exactly_and_recovers_displays() {
        let frame = Geometry {
            x: 123.25,
            y: 156.5,
            width: 472.5,
            height: 720.0,
        };
        assert_eq!(frame.restored(&[screen()]), frame);
        let second = Geometry {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let remote = Geometry {
            x: -1400.0,
            ..frame
        };
        assert_eq!(remote.restored(&[screen(), second]), remote);
        let recovered = remote.restored(&[screen()]);
        assert!(recovered.x >= 0.0 && recovered.y >= 24.0);
        for width in [10.0, 315.0, 420.0, 630.0, 5000.0] {
            let f = Geometry {
                x: 1400.0,
                y: 890.0,
                width,
                ..frame
            }
            .restored(&[screen()]);
            assert!((MIN_WIDTH..=MAX_WIDTH).contains(&f.width));
            assert!((f.width / f.height - RATIO).abs() < 1e-9);
            assert!(f.x + f.width <= 1440.0 && f.y + f.height <= 900.0);
        }
        let reset = frame.canonical().restored(&[screen()]);
        assert_eq!((reset.width, reset.height), (420.0, 640.0));
        assert_eq!(reset.y + reset.height, frame.y + frame.height);
        let tiny = Geometry {
            width: 300.0,
            height: 400.0,
            ..screen()
        };
        let f = frame.restored(&[tiny]);
        assert_eq!(f.width, MIN_WIDTH);
        assert_eq!(f.y + f.height, tiny.y + tiny.height);
        assert_eq!(
            Geometry {
                width: f64::NAN,
                ..frame
            }
            .restored(&[])
            .width,
            420.0
        );
    }
    #[test]
    fn preferences_are_atomic_debounced_and_preserve_pin_and_unknown_keys() {
        let dir = std::env::temp_dir().join(format!("ipod-window-{:x}", rand::random::<u64>()));
        let path = dir.join("window.json");
        let mut store = Store::load(Some(path.clone()));
        assert!(!store.preferences.always_on_top);
        assert!(store.preferences.geometry.is_none());
        let now = Instant::now();
        store.record(screen().canonical(), now);
        assert!(!store.due(now + Duration::from_millis(500)));
        store.record(
            Geometry {
                x: 10.0,
                ..screen().canonical()
            },
            now + Duration::from_millis(500),
        );
        assert!(!store.due(now + Duration::from_millis(1000)));
        assert!(store.due(now + Duration::from_millis(1250)));
        store.pin(true);
        store
            .preferences
            .extra
            .insert("future".into(), serde_json::json!(42));
        store.flush().unwrap();
        assert_eq!(
            Store::load(Some(path.clone())).preferences,
            store.preferences
        );
        assert!(!store.due(now + Duration::from_secs(10)));
        store.record(screen().canonical(), now);
        store.flush().unwrap();
        assert!(Store::load(Some(path.clone())).preferences.always_on_top);
        fs::write(&path, b"corrupt").unwrap();
        assert_eq!(Store::load(Some(path)).preferences, Preferences::default());
        fs::remove_dir_all(dir).unwrap();
    }
}
