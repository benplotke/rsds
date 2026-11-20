//! The configuration to go back to while the UI asks "Keep these display
//! settings?" (`$XDG_RUNTIME_DIR/rsds-unconfirmed.json`).
//!
//! If monitors are connected or disconnected during the countdown, the
//! daemon takes this file and handles the change starting from this
//! configuration, as if the countdown had run out first. The file records
//! when the countdown ends, so one left behind by a UI that died is ignored.

use std::{
    io,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::OutputConfig;

#[derive(Serialize, Deserialize)]
struct Unconfirmed {
    /// End of the countdown, in seconds since the Unix epoch.
    until: u64,
    previous: Vec<OutputConfig>,
}

fn path() -> std::path::PathBuf {
    crate::daemon_lock::runtime_file("rsds-unconfirmed.json")
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Record `previous` as the configuration to go back to for `countdown`.
pub fn save(previous: &[OutputConfig], countdown: Duration) -> io::Result<()> {
    let u = Unconfirmed { until: unix_now() + countdown.as_secs() + 1, previous: previous.to_vec() };
    crate::config::write_atomic(&path(), &serde_json::to_vec(&u).map_err(io::Error::other)?)
}

/// The countdown ended: forget the configuration.
pub fn clear() {
    let _ = std::fs::remove_file(path());
}

/// Take the configuration to go back to, if a countdown is running.
pub fn take() -> Option<Vec<OutputConfig>> {
    let bytes = std::fs::read(path()).ok()?;
    clear();
    let u: Unconfirmed = serde_json::from_slice(&bytes).ok()?;
    (u.until >= unix_now()).then_some(u.previous)
}
