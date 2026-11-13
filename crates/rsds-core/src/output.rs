use serde::{Deserialize, Serialize};

/// A display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    /// Refresh rate in mHz (0 if unknown).
    pub refresh_mhz: i32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preferred: bool,
}

impl Mode {
    pub fn refresh_hz(&self) -> f64 {
        self.refresh_mhz as f64 / 1000.0
    }

    /// Same resolution and refresh, ignoring the preferred flag.
    pub fn same_as(&self, other: &Mode) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.refresh_mhz == other.refresh_mhz
    }
}

/// Output orientation. Matches the wl_output transform values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transform {
    #[default]
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    /// True if width and height are swapped on screen.
    pub fn is_rotated(self) -> bool {
        matches!(
            self,
            Transform::Rotate90 | Transform::Rotate270 | Transform::Flipped90 | Transform::Flipped270
        )
    }
}

/// A connected output (monitor) and its current state.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// Connector name, e.g. "eDP-1", "DP-3".
    pub name: String,
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
    /// Physical size in millimetres, if known.
    pub physical_size_mm: Option<(i32, i32)>,
    pub modes: Vec<Mode>,
    pub enabled: bool,
    /// Index into `modes` of the current mode, if enabled.
    pub current_mode: Option<usize>,
    /// Position of the top-left corner in the global layout, logical pixels.
    pub position: (i32, i32),
    pub transform: Transform,
    pub scale: f64,
}

impl Output {
    /// Built-in laptop panel.
    pub fn is_internal(&self) -> bool {
        ["eDP", "LVDS", "DSI"].iter().any(|p| self.name.starts_with(p))
    }

    /// Stable identity used to recognise this monitor across reconnects.
    /// Uses EDID make/model/serial; falls back to the connector name when
    /// there is no serial (identical monitors would otherwise collide).
    pub fn identity(&self) -> String {
        match &self.serial {
            Some(serial) if !serial.is_empty() => {
                format!("{}|{}|{}", self.make, self.model, serial)
            }
            _ => format!("{}|{}|@{}", self.make, self.model, self.name),
        }
    }

    pub fn mode(&self) -> Option<&Mode> {
        self.current_mode.and_then(|i| self.modes.get(i))
    }

    pub fn preferred_mode(&self) -> Option<&Mode> {
        self.modes.iter().find(|m| m.preferred).or(self.modes.first())
    }

    /// Size in the global layout (after transform and scale), if enabled.
    pub fn logical_size(&self) -> Option<(i32, i32)> {
        if !self.enabled {
            return None;
        }
        let mode = self.mode()?;
        let (w, h) = if self.transform.is_rotated() {
            (mode.height, mode.width)
        } else {
            (mode.width, mode.height)
        };
        Some((
            (w as f64 / self.scale).round() as i32,
            (h as f64 / self.scale).round() as i32,
        ))
    }

    /// The configuration that reproduces this output's current state.
    pub fn config(&self) -> OutputConfig {
        OutputConfig {
            name: self.name.clone(),
            enabled: self.enabled,
            mode: self.mode().copied(),
            position: self.position,
            transform: self.transform,
            scale: self.scale,
        }
    }
}

impl OutputConfig {
    /// Rectangle in the global layout, if enabled with a mode.
    pub fn rect(&self) -> Option<crate::layout::Rect> {
        let mode = self.mode.filter(|_| self.enabled)?;
        let (w, h) = if self.transform.is_rotated() {
            (mode.height, mode.width)
        } else {
            (mode.width, mode.height)
        };
        Some(crate::layout::Rect::new(
            self.position.0,
            self.position.1,
            (w as f64 / self.scale).round() as i32,
            (h as f64 / self.scale).round() as i32,
        ))
    }
}

/// Desired configuration for one output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Connector name.
    pub name: String,
    pub enabled: bool,
    /// Required when enabled.
    pub mode: Option<Mode>,
    pub position: (i32, i32),
    pub transform: Transform,
    pub scale: f64,
}
