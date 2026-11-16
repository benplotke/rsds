//! Saved configurations per set of connected monitors
//! (`~/.config/rsds/profiles.json`).

use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{Output, OutputConfig};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Profiles {
    pub profiles: Vec<Profile>,
}

/// Configuration for one particular set of connected monitors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// Sorted identities of the monitors in this set (see `Output::identity`).
    pub monitors: Vec<String>,
    pub outputs: Vec<SavedOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedOutput {
    pub identity: String,
    /// `config.name` is the connector at the time of saving; it is replaced
    /// by the monitor's current connector when the profile is applied.
    #[serde(flatten)]
    pub config: OutputConfig,
    /// Identity of the monitor this one duplicates, if any. Only needed to
    /// tell which is which when both are the same size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<String>,
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
    /// The configuration to apply to `outputs`, with connector names
    /// resolved. None if the profile doesn't cover every output.
    pub fn configs(&self, outputs: &[Output]) -> Option<Vec<OutputConfig>> {
        outputs
            .iter()
            .map(|o| {
                let saved = self.outputs.iter().find(|s| s.identity == o.identity())?;
                Some(OutputConfig { name: o.name.clone(), ..saved.config.clone() })
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
                let source = saved.duplicates.as_ref()?;
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

/// `$XDG_CONFIG_HOME/rsds`, defaulting to `~/.config/rsds`.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rsds"))
}

/// Write a file atomically: to a temporary file, then rename over the old
/// one, so readers never see a half-written file.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

impl Profiles {
    /// `$XDG_CONFIG_HOME/rsds/profiles.json`, defaulting to `~/.config`.
    pub fn path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("profiles.json"))
    }

    /// Load the saved profiles. A missing file is an empty set of profiles.
    pub fn load() -> io::Result<Self> {
        let path = Self::path().ok_or_else(|| io::Error::other("no home directory"))?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::other(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path().ok_or_else(|| io::Error::other("no home directory"))?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        write_atomic(path, &serde_json::to_vec_pretty(self).map_err(io::Error::other)?)
    }

    /// The profile for exactly this set of monitors.
    pub fn find(&self, outputs: &[Output]) -> Option<&Profile> {
        let set = monitor_set(outputs);
        self.profiles.iter().find(|p| p.monitors == set)
    }

    /// Record the current state of `outputs` as the profile for their set.
    /// `hint` says which output each one duplicates, where known (see
    /// [`duplicate_sources`]).
    pub fn remember(&mut self, outputs: &[Output], hint: &[Option<usize>]) {
        let sources = duplicate_sources(outputs, hint);
        let profile = Profile {
            monitors: monitor_set(outputs),
            outputs: outputs
                .iter()
                .zip(sources)
                .map(|(o, source)| SavedOutput {
                    identity: o.identity(),
                    config: o.config(),
                    duplicates: source.map(|s| outputs[s].identity()),
                })
                .collect(),
        };
        match self.profiles.iter_mut().find(|p| p.monitors == profile.monitors) {
            Some(p) => *p = profile,
            None => self.profiles.push(profile),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mode, Transform};

    fn output(name: &str, serial: &str, x: i32) -> Output {
        Output {
            name: name.into(),
            make: "Make".into(),
            model: "Model".into(),
            serial: Some(serial.into()),
            physical_size_mm: None,
            modes: vec![Mode { width: 1920, height: 1080, refresh_mhz: 60000, preferred: true }],
            enabled: true,
            current_mode: Some(0),
            position: (x, 0),
            transform: Transform::Normal,
            scale: 1.0,
        }
    }

    #[test]
    fn round_trip_and_connector_remap() {
        let dir = std::env::temp_dir().join(format!("rsds-test-{}", std::process::id()));
        let path = dir.join("profiles.json");
        let mut p = Profiles::default();
        p.remember(&[output("DP-1", "A", 0), output("DP-2", "B", 1920)], &[]);
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
        p.remember(&[output("DP-1", "A", 0)], &[]);
        p.remember(&[output("DP-1", "A", 100)], &[]);
        assert_eq!(p.profiles.len(), 1);
        assert_eq!(p.profiles[0].outputs[0].config.position, (100, 0));
    }

    #[test]
    fn remembers_which_duplicates_which() {
        // Same size, so only the hint can tell: B duplicates A.
        let outputs = [output("DP-1", "A", 0), output("DP-2", "B", 0)];
        let mut p = Profiles::default();
        p.remember(&outputs, &[Some(1), None]);
        let profile = p.find(&outputs).unwrap();
        assert_eq!(profile.duplicate_sources(&outputs), vec![Some(1), None]);
    }

    #[test]
    fn missing_file_is_empty() {
        let p = Profiles::load_from(Path::new("/nonexistent/rsds/profiles.json")).unwrap();
        assert!(p.profiles.is_empty());
    }
}
