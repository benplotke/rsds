//! Saved configurations per set of connected monitors
//! (`~/.config/rsds/profiles.json`).

use serde::{Deserialize, Serialize};

use crate::{Mode, Output, OutputConfig, Transform, config::ConfigFile, layout::Rect};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Profiles {
    pub profiles: Vec<Profile>,
}

impl ConfigFile for Profiles {
    const NAME: &'static str = "profiles.json";
}

/// Configuration for one particular set of connected monitors: the ones
/// in `outputs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub outputs: Vec<SavedOutput>,
    /// Places to go back to for monitors that are off or duplicating.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restores: Vec<SavedRestore>,
}

/// One monitor in a profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "SavedOutputFile", into = "SavedOutputFile")]
pub struct SavedOutput {
    /// See [`Output::identity`].
    pub identity: String,
    /// None if it is off.
    pub on: Option<SavedOn>,
}

/// How a monitor that is on is configured.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedOn {
    pub mode: Mode,
    pub position: (i32, i32),
    pub transform: Transform,
    pub scale: f64,
    /// Identity of the monitor this one duplicates, if any. Only needed to
    /// tell which is which when both are the same size.
    pub duplicates: Option<String>,
    /// The primary monitor (X11).
    pub primary: bool,
}

/// [`SavedOutput`] as written: `enabled`, and the rest only when it is on.
#[derive(Serialize, Deserialize)]
struct SavedOutputFile {
    identity: String,
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<SavedMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<(i32, i32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transform: Option<Transform>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duplicates: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    primary: bool,
}

/// A mode as written: without whether it is the monitor's preferred one.
#[derive(Serialize, Deserialize)]
struct SavedMode {
    width: i32,
    height: i32,
    refresh_mhz: i32,
}

impl TryFrom<SavedOutputFile> for SavedOutput {
    type Error = String;

    fn try_from(f: SavedOutputFile) -> Result<Self, String> {
        let on = match (f.enabled, f.mode, f.position) {
            (false, ..) => None,
            (true, Some(m), Some(position)) => Some(SavedOn {
                mode: Mode { width: m.width, height: m.height, refresh_mhz: m.refresh_mhz, preferred: false },
                position,
                transform: f.transform.unwrap_or_default(),
                scale: f.scale.unwrap_or(1.0),
                duplicates: f.duplicates,
                primary: f.primary,
            }),
            (true, ..) => return Err(format!("{}: a monitor that is on needs a mode and a position", f.identity)),
        };
        Ok(SavedOutput { identity: f.identity, on })
    }
}

impl From<SavedOutput> for SavedOutputFile {
    fn from(s: SavedOutput) -> Self {
        let on = s.on;
        SavedOutputFile {
            identity: s.identity,
            enabled: on.is_some(),
            mode: on.as_ref().map(|o| SavedMode {
                width: o.mode.width,
                height: o.mode.height,
                refresh_mhz: o.mode.refresh_mhz,
            }),
            position: on.as_ref().map(|o| o.position),
            transform: on.as_ref().map(|o| o.transform),
            scale: on.as_ref().map(|o| o.scale),
            primary: on.as_ref().is_some_and(|o| o.primary),
            duplicates: on.and_then(|o| o.duplicates),
        }
    }
}

/// The arrangement before a monitor left it (was turned off or made a
/// duplicate), so it can go back to its place when it returns, as long as
/// the others haven't been changed since.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedRestore {
    /// Identity of the monitor that left.
    pub identity: String,
    /// The monitors that were on before it left.
    pub before: Vec<SavedPlace>,
    /// The other monitors that were on right after it left.
    pub after: Vec<SavedArea>,
}

/// Where a monitor was, to put it back there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedPlace {
    pub identity: String,
    pub position: (i32, i32),
    pub scale: f64,
    /// Identity of the monitor it duplicated, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<String>,
}

/// Where a monitor was and how big, to tell whether it has changed since.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedArea {
    pub identity: String,
    pub position: (i32, i32),
    pub size: (i32, i32),
    /// Identity of the monitor it duplicated, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<String>,
}

impl SavedArea {
    pub fn rect(&self) -> Rect {
        Rect::new(self.position.0, self.position.1, self.size.0, self.size.1)
    }
}

/// Sorted identities of a set of outputs.
pub fn monitor_set(outputs: &[Output]) -> Vec<String> {
    let mut ids: Vec<String> = outputs.iter().map(Output::identity).collect();
    ids.sort();
    ids
}

/// The outputs that make up the monitor set. With the lid closed and an
/// external monitor connected, the built-in panel doesn't count: it is
/// turned off, and that situation has its own profile.
pub fn active_outputs(outputs: &[Output], lid_closed: bool) -> Vec<Output> {
    let docked = lid_closed && outputs.iter().any(|o| !o.is_internal());
    outputs.iter().filter(|o| !(docked && o.is_internal())).cloned().collect()
}

impl Profile {
    /// Sorted identities of the monitors in this set.
    pub fn monitors(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.outputs.iter().map(|s| s.identity.clone()).collect();
        ids.sort();
        ids
    }

    /// The configuration to apply to `outputs`, with connector names
    /// resolved. None if the profile doesn't cover every output.
    /// A profile without a primary monitor (as saved on Wayland) leaves
    /// the primary monitor as it is.
    pub fn configs(&self, outputs: &[Output]) -> Option<Vec<OutputConfig>> {
        let has_primary = self.outputs.iter().any(|s| s.on.as_ref().is_some_and(|on| on.primary));
        outputs
            .iter()
            .map(|o| {
                let saved = self.outputs.iter().find(|s| s.identity == o.identity())?;
                Some(match &saved.on {
                    // Off: the rest of its state doesn't matter.
                    None => OutputConfig { enabled: false, primary: false, ..o.config() },
                    Some(on) => OutputConfig {
                        name: o.name.clone(),
                        enabled: true,
                        // The monitor's own mode, which knows whether it is the
                        // preferred one.
                        mode: Some(*o.modes.iter().find(|m| m.same_as(&on.mode)).unwrap_or(&on.mode)),
                        position: on.position,
                        transform: on.transform,
                        scale: on.scale,
                        primary: if has_primary { on.primary } else { o.primary },
                    },
                })
            })
            .collect()
    }

    /// For each of `outputs`, the index of the output it duplicates
    /// according to this profile.
    pub fn duplicate_sources(&self, outputs: &[Output]) -> Vec<Option<usize>> {
        outputs
            .iter()
            .map(|o| {
                let saved = self.outputs.iter().find(|s| s.identity == o.identity())?;
                let source = saved.on.as_ref()?.duplicates.as_ref()?;
                outputs.iter().position(|x| &x.identity() == source)
            })
            .collect()
    }
}

/// For each of `outputs`, the index of the output it duplicates (see
/// [`crate::mirror::infer`]).
pub fn duplicate_sources(outputs: &[Output], hint: &[Option<usize>]) -> Vec<Option<usize>> {
    let rects: Vec<_> = outputs.iter().map(|o| o.config().rect()).collect();
    crate::mirror::infer(&rects, hint)
}

impl Profiles {
    /// The profile for exactly this set of monitors.
    pub fn find(&self, outputs: &[Output]) -> Option<&Profile> {
        let set = monitor_set(outputs);
        self.profiles.iter().find(|p| p.monitors() == set)
    }

    /// Record the current state of `outputs` as the profile for their set.
    /// `hint` says which output each one duplicates, where known (see
    /// [`duplicate_sources`]). `restores` are the places to go back to.
    pub fn remember(&mut self, outputs: &[Output], hint: &[Option<usize>], restores: Vec<SavedRestore>) {
        let sources = duplicate_sources(outputs, hint);
        let profile = Profile {
            outputs: outputs
                .iter()
                .zip(sources)
                .map(|(o, source)| SavedOutput {
                    identity: o.identity(),
                    on: o.mode().filter(|_| o.enabled).map(|mode| SavedOn {
                        mode: *mode,
                        position: o.position,
                        transform: o.transform,
                        scale: o.scale,
                        duplicates: source.map(|s| outputs[s].identity()),
                        primary: o.primary,
                    }),
                })
                .collect(),
            restores,
        };
        let set = monitor_set(outputs);
        match self.profiles.iter_mut().find(|p| p.monitors() == set) {
            Some(p) => *p = profile,
            None => self.profiles.push(profile),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn output(name: &str, serial: &str, x: i32) -> Output {
        Output {
            name: name.into(),
            make: "Make".into(),
            model: "Model".into(),
            serial: Some(serial.into()),
            modes: vec![Mode { width: 1920, height: 1080, refresh_mhz: 60000, preferred: true }],
            enabled: true,
            current_mode: Some(0),
            position: (x, 0),
            transform: Transform::Normal,
            scale: 1.0,
            primary: false,
        }
    }

    #[test]
    fn round_trip_and_connector_remap() {
        let dir = std::env::temp_dir().join(format!("rsds-test-{}", std::process::id()));
        let path = dir.join("profiles.json");
        let mut p = Profiles::default();
        p.remember(&[output("DP-1", "A", 0), output("DP-2", "B", 1920)], &[], Vec::new());
        p.save_to(&path).unwrap();
        let loaded = Profiles::load_from(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        // Same monitors on swapped connectors, in a different order.
        let now = [output("DP-7", "B", 0), output("DP-6", "A", 0)];
        let configs = loaded.find(&now).unwrap().configs(&now).unwrap();
        assert_eq!(configs[0].name, "DP-7");
        assert_eq!(configs[0].position, (1920, 0));
        assert_eq!(configs[1].name, "DP-6");
        assert_eq!(configs[1].position, (0, 0));
    }

    #[test]
    fn remember_replaces_same_set() {
        let mut p = Profiles::default();
        p.remember(&[output("DP-1", "A", 0)], &[], Vec::new());
        p.remember(&[output("DP-1", "A", 100)], &[], Vec::new());
        assert_eq!(p.profiles.len(), 1);
        assert_eq!(p.profiles[0].outputs[0].on.as_ref().unwrap().position, (100, 0));
    }

    #[test]
    fn remembers_which_duplicates_which() {
        // Same size, so only the hint can tell: B duplicates A.
        let outputs = [output("DP-1", "A", 0), output("DP-2", "B", 0)];
        let mut p = Profiles::default();
        p.remember(&outputs, &[Some(1), None], Vec::new());
        let profile = p.find(&outputs).unwrap();
        assert_eq!(profile.duplicate_sources(&outputs), vec![Some(1), None]);
    }

    #[test]
    fn missing_file_is_empty() {
        let p = Profiles::load_from(Path::new("/nonexistent/rsds/profiles.json")).unwrap();
        assert!(p.profiles.is_empty());
    }

    #[test]
    fn writes_only_what_is_used() {
        let mut off = output("DP-2", "B", 1920);
        off.enabled = false;
        off.current_mode = None;
        let outputs = [output("DP-1", "A", 0), off];
        let mut p = Profiles::default();
        p.remember(&outputs, &[], Vec::new());
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["profiles"][0]["outputs"][1], serde_json::json!({ "identity": "Make|Model|B", "enabled": false }));
        let on = &json["profiles"][0]["outputs"][0];
        assert_eq!(on["mode"], serde_json::json!({ "width": 1920, "height": 1080, "refresh_mhz": 60000 }));
        assert!(on.get("name").is_none());

        // Read back, the mode is the monitor's own (preferred) one again,
        // and the monitor that is off stays as it is.
        let loaded: Profiles = serde_json::from_value(json).unwrap();
        let configs = loaded.find(&outputs).unwrap().configs(&outputs).unwrap();
        assert_eq!(configs[0], outputs[0].config());
        assert_eq!(configs[1], outputs[1].config());
    }
}
