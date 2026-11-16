//! The lid in the daemon: follows the lid (via logind) and the Lid Close
//! settings, holds logind's lid inhibitor so that the daemon decides what
//! closing the lid does (except with System default, which leaves the lid
//! to logind), and suspends or turns the panel off accordingly.

use std::os::fd::BorrowedFd;

use rsds_core::{
    Backend, Output,
    lid_close::{LidClose, LidCloseSettings},
};
use rsds_logind::lid::{Inhibitor, Lid};

use crate::log;

/// What the lid calls for right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// The machine should be suspended.
    pub suspend: bool,
    /// The built-in panel is turned off and left out of the monitor set.
    pub panel_out_of_set: bool,
    /// The built-in panel stays in the layout but its display is off.
    pub panel_dark: bool,
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
    Policy {
        suspend,
        panel_out_of_set: closed && docked && setting != LidClose::Suspend,
        panel_dark: closed && !docked && setting == LidClose::Never,
    }
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
        if p.panel_dark != self.panel_dark {
            self.panel_dark = p.panel_dark;
            for o in outputs.iter().filter(|o| o.is_internal() && o.enabled) {
                match backend.set_power(&o.name, !p.panel_dark) {
                    Ok(()) => log!("{} display {}", o.name, if p.panel_dark { "off" } else { "on" }),
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

    #[test]
    fn always_suspend() {
        let p = policy(true, true, LidClose::Suspend);
        assert!(p.suspend && !p.panel_out_of_set && !p.panel_dark);
        assert!(policy(true, false, LidClose::Suspend).suspend);
    }

    #[test]
    fn suspend_unless_docked() {
        let docked = policy(true, true, LidClose::SuspendUnlessDocked);
        assert!(!docked.suspend && docked.panel_out_of_set && !docked.panel_dark);
        assert!(policy(true, false, LidClose::SuspendUnlessDocked).suspend);
    }

    #[test]
    fn never_suspend() {
        let docked = policy(true, true, LidClose::Never);
        assert!(!docked.suspend && docked.panel_out_of_set && !docked.panel_dark);
        let alone = policy(true, false, LidClose::Never);
        assert!(!alone.suspend && !alone.panel_out_of_set && alone.panel_dark);
    }

    #[test]
    fn system_default() {
        let docked = policy(true, true, LidClose::SystemDefault);
        assert!(!docked.suspend && docked.panel_out_of_set && !docked.panel_dark);
        let alone = policy(true, false, LidClose::SystemDefault);
        assert!(!alone.suspend && !alone.panel_out_of_set && !alone.panel_dark);
    }

    #[test]
    fn open_lid_does_nothing() {
        for s in LidClose::ALL {
            for docked in [false, true] {
                assert_eq!(
                    policy(false, docked, s),
                    Policy { suspend: false, panel_out_of_set: false, panel_dark: false }
                );
            }
        }
    }
}
