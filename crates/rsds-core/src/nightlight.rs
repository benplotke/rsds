//! Nightlight settings (`~/.config/rsds/nightlight.json`) and colour maths.
//!
//! The UI edits the settings file; the daemon watches it and applies the
//! colour temperature through the backend.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Neutral colour temperature: no tint.
pub const NEUTRAL_K: u32 = 6500;
/// Temperature range offered by the UI, in kelvin.
pub const MIN_K: u32 = 1900;
pub const MAX_K: u32 = 6500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Nightlight {
    /// Whether the tint is on now. The schedule turns it on at `start`
    /// and off at `end`; in between, it can be switched either way.
    pub enabled: bool,
    /// Colour temperature while active, in kelvin.
    pub temperature: u32,
    /// Turn on at `start` and off at `end` each day.
    pub schedule: bool,
    pub start: TimeOfDay,
    pub end: TimeOfDay,
}

impl Default for Nightlight {
    fn default() -> Self {
        Self {
            enabled: false,
            temperature: 4000,
            schedule: false,
            start: TimeOfDay(21 * 60),
            end: TimeOfDay(7 * 60),
        }
    }
}

impl crate::config::ConfigFile for Nightlight {
    const NAME: &'static str = "nightlight.json";
}

/// Minutes after local midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeOfDay(pub u16);

impl TimeOfDay {
    pub const MINUTES_PER_DAY: u16 = 24 * 60;

    pub fn hour(self) -> u16 {
        self.0 / 60
    }

    pub fn minute(self) -> u16 {
        self.0 % 60
    }

}

impl std::fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.hour(), self.minute())
    }
}

impl std::str::FromStr for TimeOfDay {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (h, m) = s.split_once(':').ok_or_else(|| format!("bad time {s:?}, expected HH:MM"))?;
        let h: u16 = h.parse().map_err(|_| format!("bad hour in {s:?}"))?;
        let m: u16 = m.parse().map_err(|_| format!("bad minute in {s:?}"))?;
        if h >= 24 || m >= 60 {
            return Err(format!("time out of range: {s:?}"));
        }
        Ok(TimeOfDay(h * 60 + m))
    }
}

// Stored as "HH:MM" so the file is easy to read and edit.
impl Serialize for TimeOfDay {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

/// RGB multipliers (0..=1) for a colour temperature, relative to 6500 K
/// white. Approximation of the black-body curve (Tanner Helland's fit).
pub fn whitepoint(kelvin: u32) -> [f64; 3] {
    fn raw(kelvin: u32) -> [f64; 3] {
        let t = kelvin.clamp(1000, 40000) as f64 / 100.0;
        let r = if t <= 66.0 { 255.0 } else { 329.698727446 * (t - 60.0).powf(-0.1332047592) };
        let g = if t <= 66.0 {
            99.4708025861 * t.ln() - 161.1195681661
        } else {
            288.1221695283 * (t - 60.0).powf(-0.0755148492)
        };
        let b = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            138.5177312231 * (t - 10.0).ln() - 305.0447927307
        };
        [r, g, b]
    }
    let w = raw(NEUTRAL_K);
    let c = raw(kelvin);
    [0, 1, 2].map(|i| (c[i] / w[i]).clamp(0.0, 1.0))
}

/// A gamma ramp of `size` entries per channel, laid out red, green, blue,
/// as wlr-gamma-control and XRandR expect.
pub fn gamma_ramp(size: usize, kelvin: u32) -> Vec<u16> {
    let wp = whitepoint(kelvin);
    let mut ramp = Vec::with_capacity(size * 3);
    for channel in wp {
        for i in 0..size {
            let v = if size > 1 { i as f64 / (size - 1) as f64 } else { 1.0 };
            ramp.push((v * channel * 65535.0).round() as u16);
        }
    }
    ramp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> TimeOfDay {
        s.parse().unwrap()
    }

    #[test]
    fn neutral_is_white_and_warm_is_red() {
        assert_eq!(whitepoint(NEUTRAL_K), [1.0, 1.0, 1.0]);
        let [r, g, b] = whitepoint(3000);
        assert_eq!(r, 1.0);
        assert!(g < 1.0 && b < g, "{g} {b}");
    }

    #[test]
    fn ramp_layout() {
        let ramp = gamma_ramp(256, NEUTRAL_K);
        assert_eq!(ramp.len(), 768);
        assert_eq!((ramp[0], ramp[255], ramp[511], ramp[767]), (0, 65535, 65535, 65535));
    }

    #[test]
    fn json_round_trip() {
        let n = Nightlight { start: t("20:15"), ..Default::default() };
        let json = serde_json::to_string(&n).unwrap();
        assert!(json.contains("\"20:15\""), "{json}");
        assert_eq!(serde_json::from_str::<Nightlight>(&json).unwrap(), n);
        assert!(serde_json::from_str::<TimeOfDay>("\"25:00\"").is_err());
    }
}
