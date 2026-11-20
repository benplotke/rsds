//! Lid Close settings (`~/.config/rsds/lid-close.json`).
//!
//! The UI edits the settings file; the daemon watches it and acts on the
//! lid accordingly.

use serde::{Deserialize, Serialize};

/// What closing the lid does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LidClose {
    /// Always suspend.
    Suspend,
    /// Suspend, unless an external monitor is connected: then turn the
    /// built-in panel off and carry on with the external monitors.
    #[default]
    SuspendUnlessDocked,
    /// Never suspend. Without an external monitor, the built-in panel is
    /// turned off; with one, as for `SuspendUnlessDocked`.
    Never,
    /// Leave suspending (and the lid switch) to the system, e.g. logind's
    /// `HandleLidSwitch` settings; with an external monitor, still turn the
    /// built-in panel off as for `SuspendUnlessDocked`.
    SystemDefault,
}

impl LidClose {
    pub const ALL: [LidClose; 4] =
        [LidClose::Suspend, LidClose::SuspendUnlessDocked, LidClose::Never, LidClose::SystemDefault];

    pub fn label(self) -> &'static str {
        match self {
            LidClose::Suspend => "Always suspend",
            LidClose::SuspendUnlessDocked => "Suspend unless docked",
            LidClose::Never => "Never suspend",
            LidClose::SystemDefault => "System default",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LidCloseSettings {
    pub lid_close: LidClose,
}

impl crate::config::ConfigFile for LidCloseSettings {
    const NAME: &'static str = "lid-close.json";
}
