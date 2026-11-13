//! Built-in panel brightness.
//!
//! Reads the current level from sysfs (world readable) and sets it through
//! logind's `Session.SetBrightness`, which needs no root or udev rules.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

const SYSFS: &str = "/sys/class/backlight";
const TIMEOUT: Duration = Duration::from_secs(2);

pub struct Backlight {
    /// Device name under /sys/class/backlight, e.g. "intel_backlight".
    name: String,
    dir: PathBuf,
    max: u32,
    /// Opened on first use.
    bus: Option<dbus::blocking::Connection>,
}

impl Backlight {
    /// Find the panel's backlight device, preferring firmware over platform
    /// over raw interfaces (the same order systemd uses).
    pub fn find() -> Option<Self> {
        Self::find_in(Path::new(SYSFS))
    }

    fn find_in(root: &Path) -> Option<Self> {
        let rank = |t: &str| match t {
            "firmware" => 0,
            "platform" => 1,
            "raw" => 2,
            _ => 3,
        };
        let mut best: Option<(u8, Self)> = None;
        for entry in std::fs::read_dir(root).ok()?.flatten() {
            let dir = entry.path();
            let kind = read_trimmed(&dir.join("type")).unwrap_or_default();
            let Some(max) = read_trimmed(&dir.join("max_brightness")).and_then(|s| s.parse().ok()) else {
                continue;
            };
            if max == 0 {
                continue;
            }
            let r = rank(&kind);
            if best.as_ref().is_none_or(|(br, _)| r < *br) {
                let name = entry.file_name().to_string_lossy().into_owned();
                best = Some((r, Self { name, dir, max, bus: None }));
            }
        }
        best.map(|(_, b)| b)
    }

    /// Current brightness in percent.
    pub fn percent(&self) -> io::Result<u8> {
        let raw: u32 = read_trimmed(&self.dir.join("brightness"))
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| io::Error::other("can't read brightness"))?;
        Ok(((raw as f64 * 100.0 / self.max as f64).round() as u32).min(100) as u8)
    }

    /// Set brightness in percent. 0% maps to the lowest non-zero level so
    /// the panel never switches off entirely.
    pub fn set_percent(&mut self, percent: u8) -> io::Result<()> {
        let raw = ((percent.min(100) as f64 / 100.0 * self.max as f64).round() as u32).max(1);
        if self.bus.is_none() {
            self.bus = Some(dbus::blocking::Connection::new_system().map_err(io::Error::other)?);
        }
        let bus = self.bus.as_ref().expect("connected above");
        let session = bus.with_proxy("org.freedesktop.login1", "/org/freedesktop/login1/session/auto", TIMEOUT);
        session
            .method_call::<(), _, _, _>(
                "org.freedesktop.login1.Session",
                "SetBrightness",
                ("backlight", self.name.as_str(), raw),
            )
            .map_err(io::Error::other)
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(root: &Path, name: &str, kind: &str, max: u32, cur: u32) {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("type"), format!("{kind}\n")).unwrap();
        std::fs::write(d.join("max_brightness"), format!("{max}\n")).unwrap();
        std::fs::write(d.join("brightness"), format!("{cur}\n")).unwrap();
    }

    #[test]
    fn prefers_firmware_and_reads_percent() {
        let root = std::env::temp_dir().join(format!("rsds-bl-{}", std::process::id()));
        device(&root, "intel_backlight", "raw", 24242, 12121);
        device(&root, "acpi_video0", "firmware", 100, 30);
        let b = Backlight::find_in(&root).unwrap();
        assert_eq!(b.name, "acpi_video0");
        assert_eq!(b.percent().unwrap(), 30);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_devices() {
        assert!(Backlight::find_in(Path::new("/nonexistent")).is_none());
    }
}
