//! The lid in the daemon: follows the lid (via logind) and the Lid Close
//! settings, holds logind's lid inhibitor so that the daemon decides what
//! closing the lid does (except with System default, which leaves the lid
//! to logind), and suspends or turns the panel off accordingly.

use std::os::fd::BorrowedFd;

use rsds_core::{
    Backend, Output,
    config::ConfigFile,
    lid_close::{LidClose, LidCloseSettings},
};
use rsds_logind::lid::{Inhibitor, Lid};

use crate::log;

/// What the lid calls for right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// The machine should be suspended.
    pub suspend: bool,
    pub panel: Panel,
}

/// What happens to the built-in panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// In use as usual.
    InUse,
    /// Turned off and left out of the monitor set.
    OutOfSet,
    /// Kept in the layout, but its display is off.
    Dark,
}

/// What to do given whether the lid is closed, whether an external monitor
/// is connected, and the setting.
pub fn policy(closed: bool, docked: bool, setting: LidClose) -> Policy {
    let suspend = closed
        && match setting {
            LidClose::Suspend => true,
            LidClose::SuspendUnlessDocked => !docked,
            LidClose::Never | LidClose::SystemDefault => false,
        };
    let panel = match (closed, docked) {
        (true, true) if setting != LidClose::Suspend => Panel::OutOfSet,
        (true, false) if setting == LidClose::Never => Panel::Dark,
        _ => Panel::InUse,
    };
    Policy { suspend, panel }
}

pub(crate) struct Service {
    lid: Option<Lid>,
    /// Held unless the setting is System default; dropping it lets logind
    /// act on the lid again.
    inhibitor: Option<Inhibitor>,
    closed: bool,
    setting: LidClose,
    /// (lid closed, docked) at the last update. Suspending only happens
    /// when these change, not when the setting does.
    last: Option<(bool, bool)>,
    panel_dark: bool,
}

impl Service {
    pub(crate) fn new() -> Self {
        let lid = Lid::watch().inspect_err(|e| log!("can't watch the lid: {e}")).ok();
        let closed = lid.as_ref().is_some_and(read);
        let mut service = Self { lid, inhibitor: None, closed, setting: load(), last: None, panel_dark: false };
        service.update_inhibitor();
        service
    }

    /// Take over the lid switch from logind, or hand it back, as the
    /// setting calls for.
    fn update_inhibitor(&mut self) {
        let wanted = self.setting != LidClose::SystemDefault;
        match (&self.lid, wanted) {
            (Some(lid), true) if self.inhibitor.is_none() => {
                self.inhibitor = lid
                    .inhibit()
                    .inspect_err(|e| log!("can't take over the lid switch from logind: {e}"))
                    .ok();
            }
            (_, false) => self.inhibitor = None,
            _ => {}
        }
    }

    /// Readable when logind has news about the lid; then call `lid_changed`.
    pub(crate) fn fd(&self) -> Option<BorrowedFd<'_>> {
        self.lid.as_ref().map(Lid::fd)
    }

    pub(crate) fn lid_changed(&mut self) {
        if let Some(lid) = &self.lid {
            self.closed = read(lid);
        }
    }

    /// Something in the config directory changed: reload the setting.
    pub(crate) fn config_changed(&mut self) {
        let setting = load();
        if setting != self.setting {
            log!("lid close: {}", setting.label());
            self.setting = setting;
            self.update_inhibitor();
        }
    }

    /// The current policy for these outputs.
    pub(crate) fn policy(&self, outputs: &[Output]) -> Policy {
        policy(self.closed, outputs.iter().any(|o| !o.is_internal()), self.setting)
    }

    /// Suspend or switch the panel's display as the policy calls for.
    pub(crate) fn update<B: Backend>(&mut self, backend: &mut B, outputs: &[Output]) {
        let p = self.policy(outputs);
        let dark = p.panel == Panel::Dark;
        if dark != self.panel_dark {
            self.panel_dark = dark;
            for o in outputs.iter().filter(|o| o.is_internal() && o.enabled) {
                match backend.set_power(&o.name, !dark) {
                    Ok(()) => log!("{} display {}", o.name, if dark { "off" } else { "on" }),
                    Err(e) => log!("can't switch {} display: {e}", o.name),
                }
            }
        }

        let now = (self.closed, outputs.iter().any(|o| !o.is_internal()));
        let changed = self.last.is_some_and(|last| last != now);
        self.last = Some(now);
        if changed
            && p.suspend
            && let Some(lid) = &self.lid
        {
            log!("lid closed: suspending");
            if let Err(e) = lid.suspend() {
                log!("can't suspend: {e}");
            }
        }
    }
}

/// Whether the lid is closed; open if logind can't say.
fn read(lid: &Lid) -> bool {
    lid.is_closed().unwrap_or_else(|e| {
        log!("can't read the lid state: {e}");
        false
    })
}

fn load() -> LidClose {
    LidCloseSettings::load()
        .map(|s| s.lid_close)
        .unwrap_or_else(|e| {
            log!("can't read Lid Close settings: {e}");
            LidClose::default()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(suspend: bool, panel: Panel) -> Policy {
        Policy { suspend, panel }
    }

    #[test]
    fn always_suspend() {
        assert_eq!(policy(true, true, LidClose::Suspend), p(true, Panel::InUse));
        assert_eq!(policy(true, false, LidClose::Suspend), p(true, Panel::InUse));
    }

    #[test]
    fn suspend_unless_docked() {
        assert_eq!(policy(true, true, LidClose::SuspendUnlessDocked), p(false, Panel::OutOfSet));
        assert_eq!(policy(true, false, LidClose::SuspendUnlessDocked), p(true, Panel::InUse));
    }

    #[test]
    fn never_suspend() {
        assert_eq!(policy(true, true, LidClose::Never), p(false, Panel::OutOfSet));
        assert_eq!(policy(true, false, LidClose::Never), p(false, Panel::Dark));
    }

    #[test]
    fn system_default() {
        assert_eq!(policy(true, true, LidClose::SystemDefault), p(false, Panel::OutOfSet));
        assert_eq!(policy(true, false, LidClose::SystemDefault), p(false, Panel::InUse));
    }

    #[test]
    fn open_lid_does_nothing() {
        for s in LidClose::ALL {
            for docked in [false, true] {
                assert_eq!(policy(false, docked, s), p(false, Panel::InUse));
            }
        }
    }
}
