//! Background service: reacts to monitors being connected or disconnected.
//!
//! - If the new set of monitors has a saved profile, apply it.
//! - Otherwise, newly connected monitors are turned on at their preferred
//!   mode and placed to the right of the others; when monitors were removed,
//!   the remaining ones are slid together to close gaps; and if every
//!   monitor would be off, they are all turned on.
//! - At startup, the saved profile (if any) for the current set is applied.

use std::io;

use rsds_core::{
    Backend, Error, Output, OutputConfig,
    layout::{self, Rect},
    profile::{Profiles, monitor_set},
};
use rustix::event::{PollFd, PollFlags};

macro_rules! log {
    ($($arg:tt)*) => { eprintln!("rsds daemon: {}", format_args!($($arg)*)) };
}

pub fn run<B: Backend>(mut backend: B) -> io::Result<()> {
    // Identities of the monitors seen last time; None until the first pass.
    let mut known: Option<Vec<String>> = None;
    loop {
        let set = monitor_set(backend.outputs());
        if known.as_ref() != Some(&set) {
            log!("monitors: {}", describe(backend.outputs()));
            let profiles = Profiles::load().unwrap_or_else(|e| {
                log!("can't read profiles: {e}");
                Profiles::default()
            });
            if let Some(configs) = plan(backend.outputs(), known.as_deref(), &profiles) {
                apply(&mut backend, &configs);
            }
            known = Some(set);
        }

        let mut fds = [PollFd::from_borrowed_fd(backend.fd(), PollFlags::IN)];
        match rustix::event::poll(&mut fds, None) {
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
        backend.dispatch().map_err(io::Error::other)?;
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
}
