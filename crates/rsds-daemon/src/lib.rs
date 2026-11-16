//! Background service: reacts to monitors being connected or disconnected.
//!
//! - If the new set of monitors has a saved profile, apply it.
//! - Otherwise, newly connected monitors are turned on at their preferred
//!   mode and placed to the right of the others; when monitors were removed,
//!   the remaining ones are slid together to close gaps; and if every
//!   monitor would be off, they are all turned on.
//! - At startup, the saved profile (if any) for the current set is applied.
//! - Lid: the daemon holds logind's lid inhibitor and follows the Suspend
//!   settings (see `lid`): suspend, or (with an external monitor) turn the
//!   built-in panel off and leave it out of the monitor set, so closing or
//!   opening the lid is handled like unplugging or plugging in the panel,
//!   or (never suspending, without one) just turn the panel's display off.
//! - Nightlight: applies the colour temperature from the settings file,
//!   following its schedule (see `nightlight`).

mod lid;
mod nightlight;

use std::io;

use rsds_core::{
    Backend, Error, Output, OutputConfig,
    layout::{self, Rect},
    profile::{Profiles, active_outputs, monitor_set},
};
use rustix::event::{PollFd, PollFlags};

macro_rules! log {
    ($($arg:tt)*) => { eprintln!("rsds daemon: {}", format_args!($($arg)*)) };
}
pub(crate) use log;

pub fn run<B: Backend>(mut backend: B) -> io::Result<()> {
    let Some(_lock) = rsds_core::daemon_lock::acquire()? else {
        return Err(io::Error::other("the daemon is already running"));
    };
    let mut nightlight = nightlight::Service::new()?;
    nightlight.update(&mut backend);

    let mut lid = lid::Service::new();

    // Identities of the active monitors last time; None until the first pass.
    let mut known: Option<Vec<String>> = None;
    loop {
        let outputs = backend.outputs().to_vec();
        lid.update(&mut backend, &outputs);

        let active = active_outputs(&outputs, lid.policy(&outputs).panel_out_of_set);
        let set = monitor_set(&active);
        if known.as_ref() != Some(&set) {
            let lid_note = if active.len() < outputs.len() { " (lid closed)" } else { "" };
            log!("monitors: {}{lid_note}", describe(&active));
            let profiles = Profiles::load().unwrap_or_else(|e| {
                log!("can't read profiles: {e}");
                Profiles::default()
            });
            // Starting with the lid closed and the panel on is handled as if
            // the lid had just closed.
            let panel_on = outputs.iter().any(|o| o.enabled && !active.iter().any(|a| a.name == o.name));
            let previous = known.clone().or_else(|| panel_on.then(|| monitor_set(&outputs)));
            if let Some(configs) = plan_with_lid(&outputs, &active, previous.as_deref(), &profiles) {
                apply(&mut backend, &configs);
            }
            known = Some(set);
        }

        let (config_ready, timer_ready, lid_ready) = {
            let [config, timer] = nightlight.fds();
            let mut fds = vec![
                PollFd::from_borrowed_fd(backend.fd(), PollFlags::IN),
                PollFd::from_borrowed_fd(config, PollFlags::IN),
                PollFd::from_borrowed_fd(timer, PollFlags::IN),
            ];
            if let Some(fd) = lid.fd() {
                fds.push(PollFd::from_borrowed_fd(fd, PollFlags::IN));
            }
            match rustix::event::poll(&mut fds, None) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(e.into()),
            }
            let ready = |i: usize| fds.get(i).is_some_and(|f| !f.revents().is_empty());
            (ready(1), ready(2), ready(3))
        };
        backend.dispatch().map_err(io::Error::other)?;
        if config_ready {
            lid.config_changed();
            nightlight.config_changed(&mut backend);
        }
        if timer_ready {
            nightlight.timer_fired(&mut backend);
        }
        if lid_ready {
            lid.lid_changed();
        }
    }
}

fn describe(outputs: &[Output]) -> String {
    let names: Vec<&str> = outputs.iter().map(|o| o.name.as_str()).collect();
    names.join(", ")
}

fn apply<B: Backend>(backend: &mut B, configs: &[OutputConfig]) {
    let mut result = backend.apply(configs);
    if let Err(Error::Cancelled) = result {
        // The compositor changed something meanwhile; catch up and retry.
        let _ = backend.dispatch();
        result = backend.apply(configs);
    }
    match result {
        Ok(()) => log!("applied configuration"),
        Err(e) => log!("applying configuration failed: {e}"),
    }
}

/// Decide what configuration to apply now that the set of connected
/// monitors is `outputs` (previously `previous`, or None at startup).
/// Returns None if nothing needs to change.
pub fn plan(outputs: &[Output], previous: Option<&[String]>, profiles: &Profiles) -> Option<Vec<OutputConfig>> {
    let current: Vec<OutputConfig> = outputs.iter().map(Output::config).collect();
    let changed = |configs: Vec<OutputConfig>| (configs != current).then_some(configs);

    if let Some(configs) = profiles.find(outputs).and_then(|p| p.configs(outputs)) {
        log!("using saved profile");
        return changed(configs);
    }
    let previous = previous?;

    let added: Vec<bool> = outputs.iter().map(|o| !previous.contains(&o.identity())).collect();
    let removed = previous
        .iter()
        .any(|id| !outputs.iter().any(|o| &o.identity() == id));
    let mut configs = current.clone();

    // Close gaps left by removed monitors (among the ones that stayed).
    if removed {
        let (idx, rects): (Vec<usize>, Vec<Rect>) = configs
            .iter()
            .enumerate()
            .filter(|(i, _)| !added[*i])
            .filter_map(|(i, c)| c.rect().map(|r| (i, r)))
            .unzip();
        let origin = (
            rects.iter().map(|r| r.x).min().unwrap_or(0),
            rects.iter().map(|r| r.y).min().unwrap_or(0),
        );
        for (i, r) in idx.into_iter().zip(layout::compact(None, origin, &rects)) {
            configs[i].position = (r.x, r.y);
        }
    }

    // New monitors: preferred mode, to the right of the existing ones.
    let mut unplaced = added.clone();
    for i in (0..outputs.len()).filter(|&i| added[i]) {
        turn_on(&mut configs, outputs, i, &mut unplaced);
    }

    // Never leave every monitor off.
    if !configs.iter().any(|c| c.enabled) {
        let mut unplaced = vec![true; outputs.len()];
        for i in 0..outputs.len() {
            turn_on(&mut configs, outputs, i, &mut unplaced);
        }
    }

    let rects: Vec<Rect> = configs.iter().filter_map(OutputConfig::rect).collect();
    let (dx, dy) = layout::normalize_offset(&rects);
    for c in configs.iter_mut().filter(|c| c.enabled) {
        c.position.0 += dx;
        c.position.1 += dy;
    }
    changed(configs)
}

/// [`plan`] for the `active` outputs, with every other output (the panel
/// of a closed laptop) turned off. Returns None if nothing needs to change.
pub fn plan_with_lid(
    outputs: &[Output],
    active: &[Output],
    previous: Option<&[String]>,
    profiles: &Profiles,
) -> Option<Vec<OutputConfig>> {
    let mut configs = plan(active, previous, profiles)
        .unwrap_or_else(|| active.iter().map(Output::config).collect());
    for o in outputs.iter().filter(|o| !active.iter().any(|a| a.name == o.name)) {
        configs.push(OutputConfig { enabled: false, ..o.config() });
    }
    let current: Vec<OutputConfig> = outputs.iter().map(Output::config).collect();
    let unchanged = configs.len() == current.len() && configs.iter().all(|c| current.contains(c));
    (!unchanged).then_some(configs)
}

/// Turn output `i` on at its preferred mode, placed right of the enabled
/// outputs that are already in place (not marked in `unplaced`).
fn turn_on(configs: &mut [OutputConfig], outputs: &[Output], i: usize, unplaced: &mut [bool]) {
    let others: Vec<Rect> = configs
        .iter()
        .enumerate()
        .filter(|(j, _)| !unplaced[*j])
        .filter_map(|(_, c)| c.rect())
        .collect();
    unplaced[i] = false;
    let c = &mut configs[i];
    c.enabled = true;
    c.mode = outputs[i].preferred_mode().copied().or(c.mode);
    if c.scale <= 0.0 {
        c.scale = 1.0;
    }
    c.position = layout::place_right(&others);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsds_core::{Mode, Transform};

    fn output(name: &str, serial: &str, pos: Option<(i32, i32)>) -> Output {
        Output {
            name: name.into(),
            make: "Make".into(),
            model: "Model".into(),
            serial: Some(serial.into()),
            physical_size_mm: None,
            modes: vec![
                Mode { width: 1280, height: 720, refresh_mhz: 60000, preferred: false },
                Mode { width: 1920, height: 1080, refresh_mhz: 60000, preferred: true },
            ],
            enabled: pos.is_some(),
            current_mode: pos.map(|_| 1),
            position: pos.unwrap_or((0, 0)),
            transform: Transform::Normal,
            scale: 1.0,
        }
    }

    fn ids(outputs: &[Output]) -> Vec<String> {
        monitor_set(outputs)
    }

    #[test]
    fn new_monitor_goes_right() {
        let before = [output("eDP-1", "L", Some((0, 0)))];
        // The compositor enabled the new monitor overlapping the laptop.
        let mut new = output("DP-1", "A", Some((0, 0)));
        new.current_mode = Some(0);
        let now = [before[0].clone(), new];
        let plan = plan(&now, Some(&ids(&before)), &Profiles::default()).unwrap();
        assert_eq!(plan[0].position, (0, 0));
        assert_eq!(plan[1].position, (1920, 0));
        assert_eq!(plan[1].mode.unwrap().width, 1920, "preferred mode");
    }

    #[test]
    fn two_new_monitors_line_up() {
        let before = [output("eDP-1", "L", Some((0, 0)))];
        // The compositor put both new monitors far away.
        let now = [
            before[0].clone(),
            output("DP-1", "A", Some((9000, 0))),
            output("DP-2", "B", Some((5000, 0))),
        ];
        let plan = plan(&now, Some(&ids(&before)), &Profiles::default()).unwrap();
        assert_eq!(plan[1].position, (1920, 0));
        assert_eq!(plan[2].position, (3840, 0));
    }

    #[test]
    fn removal_closes_gap() {
        let before = [
            output("eDP-1", "L", Some((0, 0))),
            output("DP-1", "A", Some((1920, 0))),
            output("DP-2", "B", Some((3840, 0))),
        ];
        let now = [before[0].clone(), before[2].clone()];
        let plan = plan(&now, Some(&ids(&before)), &Profiles::default()).unwrap();
        assert_eq!(plan[1].position, (1920, 0));
    }

    #[test]
    fn never_all_off() {
        let before = [output("eDP-1", "L", None), output("DP-1", "A", Some((0, 0)))];
        let now = [before[0].clone()];
        let plan = plan(&now, Some(&ids(&before)), &Profiles::default()).unwrap();
        assert!(plan[0].enabled);
        assert_eq!(plan[0].mode.unwrap().width, 1920);
    }

    #[test]
    fn saved_profile_wins() {
        let saved = [output("eDP-1", "L", Some((1920, 0))), output("DP-1", "A", Some((0, 0)))];
        let mut profiles = Profiles::default();
        profiles.remember(&saved);
        let now = [output("eDP-1", "L", Some((0, 0))), output("DP-1", "A", Some((1920, 0)))];
        let plan = plan(&now, None, &profiles).unwrap();
        assert_eq!(plan[0].position, (1920, 0));
        assert_eq!(plan[1].position, (0, 0));
    }

    #[test]
    fn startup_without_profile_does_nothing() {
        let now = [output("eDP-1", "L", Some((0, 0)))];
        assert!(plan(&now, None, &Profiles::default()).is_none());
    }

    #[test]
    fn no_change_when_already_right() {
        let before = [output("eDP-1", "L", Some((0, 0)))];
        let now = [before[0].clone(), output("DP-1", "A", Some((1920, 0)))];
        assert!(plan(&now, Some(&ids(&before)), &Profiles::default()).is_none());
    }

    fn by_name<'a>(configs: &'a [OutputConfig], name: &str) -> &'a OutputConfig {
        configs.iter().find(|c| c.name == name).unwrap()
    }

    /// Plan after the lid closes, given the monitors before.
    fn close_lid(outputs: &[Output], profiles: &Profiles) -> Option<Vec<OutputConfig>> {
        let active = active_outputs(outputs, true);
        plan_with_lid(outputs, &active, Some(&ids(outputs)), profiles)
    }

    #[test]
    fn lid_close_turns_panel_off_and_keeps_externals() {
        // DP-1 above the panel, DP-2 right of DP-1.
        let outputs = [
            output("eDP-1", "L", Some((0, 1080))),
            output("DP-1", "A", Some((0, 0))),
            output("DP-2", "B", Some((1920, 0))),
        ];
        let plan = close_lid(&outputs, &Profiles::default()).unwrap();
        assert!(!by_name(&plan, "eDP-1").enabled);
        assert_eq!(by_name(&plan, "DP-1").position, (0, 0));
        assert_eq!(by_name(&plan, "DP-2").position, (1920, 0));
    }

    #[test]
    fn lid_close_closes_gap() {
        let outputs = [
            output("DP-1", "A", Some((0, 0))),
            output("eDP-1", "L", Some((1920, 0))),
            output("DP-2", "B", Some((3840, 0))),
        ];
        let plan = close_lid(&outputs, &Profiles::default()).unwrap();
        assert!(!by_name(&plan, "eDP-1").enabled);
        assert_eq!(by_name(&plan, "DP-1").position, (0, 0));
        assert_eq!(by_name(&plan, "DP-2").position, (1920, 0));
    }

    #[test]
    fn lid_close_uses_externals_profile() {
        let mut profiles = Profiles::default();
        profiles.remember(&[output("DP-1", "A", Some((1920, 0))), output("DP-2", "B", Some((0, 0)))]);
        let outputs = [
            output("eDP-1", "L", Some((0, 1080))),
            output("DP-1", "A", Some((0, 0))),
            output("DP-2", "B", Some((1920, 0))),
        ];
        let plan = close_lid(&outputs, &profiles).unwrap();
        assert!(!by_name(&plan, "eDP-1").enabled);
        assert_eq!(by_name(&plan, "DP-1").position, (1920, 0));
        assert_eq!(by_name(&plan, "DP-2").position, (0, 0));
    }

    #[test]
    fn lid_open_adds_panel_on_right() {
        let outputs = [output("eDP-1", "L", None), output("DP-1", "A", Some((0, 0)))];
        let before = active_outputs(&outputs, true);
        let plan = plan_with_lid(&outputs, &outputs, Some(&ids(&before)), &Profiles::default()).unwrap();
        let panel = by_name(&plan, "eDP-1");
        assert!(panel.enabled);
        assert_eq!(panel.position, (1920, 0));
    }

    #[test]
    fn lid_closed_without_externals_keeps_panel() {
        let outputs = [output("eDP-1", "L", Some((0, 0)))];
        assert_eq!(active_outputs(&outputs, true).len(), 1);
        assert!(close_lid(&outputs, &Profiles::default()).is_none());
    }
}
