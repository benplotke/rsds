//! Terminal UI for arranging and configuring monitors.

mod app;
mod ui;

use std::{
    io,
    time::{Duration, Instant},
};

use app::{App, Focus, Request};
use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
            KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        },
        execute,
    },
    layout::{Position, Rect},
};
use rsds_core::{
    Backend, OutputConfig,
    profile::{Profiles, active_outputs},
    lid_close::LidClose,
};
use rustix::event::{PollFd, PollFlags, Timespec};

/// Run the UI until the user quits.
pub fn run<B: Backend>(mut backend: B) -> io::Result<()> {
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let result = event_loop(&mut terminal, &mut backend);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

enum Flow {
    Continue,
    Quit,
}

fn event_loop<B: Backend>(terminal: &mut DefaultTerminal, backend: &mut B) -> io::Result<()> {
    let mut app = App::new(backend.outputs(), rsds_logind::Backlight::find());
    app.load_settings();
    // Learn whether the lid has taken the panel out of use.
    app.outputs_changed(backend.outputs(), lid_closed(&app));
    load_profile(&mut app, backend);
    let stdin = io::stdin();
    // Start crossterm's event source now so it installs its SIGWINCH
    // handler; the signal interrupts poll() below and we pick up resizes.
    event::poll(Duration::ZERO)?;

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        // While a confirmation countdown runs, wake every second to redraw it.
        let timeout = app.next_deadline().map(|d| {
            let t = d.saturating_duration_since(Instant::now()).min(Duration::from_secs(1));
            Timespec { tv_sec: t.as_secs() as _, tv_nsec: t.subsec_nanos() as _ }
        });
        {
            let mut fds = [
                PollFd::new(&stdin, PollFlags::IN),
                PollFd::from_borrowed_fd(backend.fd(), PollFlags::IN),
            ];
            match rustix::event::poll(&mut fds, timeout.as_ref()) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(e.into()),
            }
        }

        if backend.dispatch().map_err(io::Error::other)? {
            let set = app.monitor_set().to_vec();
            let request = app.outputs_changed(backend.outputs(), lid_closed(&app));
            if app.monitor_set() != set {
                load_profile(&mut app, backend);
            }
            if let Some(r) = request {
                perform(&mut app, backend, r);
            }
        }
        while event::poll(Duration::ZERO)? {
            let request = match event::read()? {
                Event::Key(key) => match handle_key(&mut app, key) {
                    Ok(r) => r,
                    Err(Flow::Quit) => return quit(&mut app, backend),
                    Err(Flow::Continue) => None,
                },
                Event::Mouse(mouse) => match handle_mouse(&mut app, mouse) {
                    Ok(r) => r,
                    Err(Flow::Quit) => return quit(&mut app, backend),
                    Err(Flow::Continue) => None,
                },
                _ => None,
            };
            if let Some(r) = request {
                perform(&mut app, backend, r);
            }
        }
        if let Some(r) = app.tick(Instant::now()) {
            perform(&mut app, backend, r);
        }
    }
}

/// Take what the saved profile for the monitors in use knows: which
/// monitor duplicates which, where they are the same size, and where
/// monitors that are off or duplicating go back to.
fn load_profile<B: Backend>(app: &mut App, backend: &B) {
    if let Ok(profiles) = Profiles::load()
        && let Some(p) = profiles.find(&active_outputs(backend.outputs(), lid_closed(app)))
    {
        app.use_profile(p);
    }
}

/// Whether the lid is closed and that takes the built-in panel out of use
/// (which it does unless closing the lid suspends).
fn lid_closed(app: &App) -> bool {
    app.lid_close.lid_close != LidClose::Suspend && rsds_logind::lid::is_closed()
}

/// Quitting during a confirmation countdown reverts, as if it ran out.
fn quit<B: Backend>(app: &mut App, backend: &mut B) -> io::Result<()> {
    if let Some(r) = app.revert() {
        perform(app, backend, r);
    }
    Ok(())
}

fn perform<B: Backend>(app: &mut App, backend: &mut B, request: Request) {
    let (configs, is_revert) = match request {
        Request::Apply(c) => (c, false),
        Request::Revert(c) => (c, true),
        Request::Identify(labels) => {
            if let Err(e) = backend.identify(&labels) {
                app.identify_until = None;
                app.status = Some(format!("Identify failed: {e}"));
            }
            return;
        }
        Request::EndIdentify => {
            backend.end_identify();
            return;
        }
        Request::SaveProfile => {
            // With the lid closed, this is the profile for the externals.
            let outputs = active_outputs(backend.outputs(), lid_closed(app));
            let hint = app.duplicate_hint(&outputs);
            let restores = app.restores_for(&outputs);
            let saved = Profiles::load().and_then(|mut p| {
                p.remember(&outputs, &hint, restores);
                p.save()
            });
            if let Err(e) = saved {
                app.status = Some(format!("Couldn't save profile: {e}"));
            }
            return;
        }
    };
    let previous: Vec<OutputConfig> = backend.outputs().iter().map(|o| o.config()).collect();
    let mut result = backend.apply(&configs);
    if let Err(rsds_core::Error::Cancelled) = result {
        // Outputs changed underneath us; catch up and try once more.
        let _ = backend.dispatch();
        result = backend.apply(&configs);
    }
    let _ = backend.dispatch();
    app.sync(backend.outputs());
    match result {
        Ok(()) => {
            // Pending now matches what was applied; adopt the live state so
            // compositor adjustments (e.g. rounding) don't show as edits.
            app.adopt_live();
            if !is_revert {
                app.applied(previous);
            }
        }
        Err(rsds_core::Error::Rejected) => app.status = Some(diagnose(backend, &configs)),
        Err(e) => app.status = Some(format!("Apply failed: {e}")),
    }
}

/// Work out which part of a rejected configuration the display server
/// objects to, by testing each monitor's change on its own.
fn diagnose<B: Backend>(backend: &mut B, configs: &[OutputConfig]) -> String {
    let mut rejected = Vec::new();
    for c in configs {
        let Some(live) = backend.outputs().iter().find(|o| o.name == c.name) else { continue };
        let current = live.config();
        if (!c.enabled && !current.enabled) || *c == current {
            continue;
        }
        let description = describe_change(&current, c);
        if let Err(rsds_core::Error::Rejected) = backend.test(std::slice::from_ref(c)) {
            rejected.push(format!("{}: {description}", c.name));
        }
    }
    if rejected.is_empty() {
        "Rejected: each change works alone, but not this combination".into()
    } else {
        format!("Rejected by display server: {}", rejected.join("; "))
    }
}

/// Short description of what differs between two configs of one output.
fn describe_change(from: &OutputConfig, to: &OutputConfig) -> String {
    if !to.enabled {
        return "turning off".into();
    }
    let mut parts = Vec::new();
    if !from.enabled {
        parts.push("turning on".to_string());
    }
    if let Some(m) = to.mode.filter(|m| from.mode.is_none_or(|f| !f.same_as(m))) {
        parts.push(format!("{} × {} @ {:.3} Hz", m.width, m.height, m.refresh_hz()));
    }
    if to.transform != from.transform {
        parts.push(app::orientation_label(to.transform).to_lowercase());
    }
    if (to.scale - from.scale).abs() > 1e-6 {
        parts.push(format!("scale {}%", (to.scale * 100.0).round()));
    }
    if to.position != from.position {
        parts.push(format!("position {},{}", to.position.0, to.position.1));
    }
    parts.join(", ")
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<Option<Request>, Flow> {
    if key.kind == KeyEventKind::Release {
        return Err(Flow::Continue);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Err(Flow::Quit);
    }
    if app.confirm.is_some() {
        return Ok(match key.code {
            KeyCode::Enter | KeyCode::Char('k') | KeyCode::Char('y') => {
                Some(app.keep())
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('r') => app.revert(),
            KeyCode::Char('q') => return Err(Flow::Quit),
            _ => None,
        });
    }
    match key.code {
        KeyCode::Char(c @ '0'..='9') => {
            app.type_digit(c as u16 - '0' as u16);
            return Ok(None);
        }
        _ => app.end_typing(),
    }
    match key.code {
        KeyCode::Char('q') => return Err(Flow::Quit),
        KeyCode::Char('a') | KeyCode::Enter => return Ok(app.apply()),
        KeyCode::Char('r') => app.reset(),
        KeyCode::Char('i') => return Ok(Some(app.identify())),
        KeyCode::Tab => app.cycle_focus(1),
        KeyCode::BackTab => app.cycle_focus(-1),
        KeyCode::Up | KeyCode::Char('k') => app.move_in_focus(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_in_focus(1),
        KeyCode::Left | KeyCode::Char('h') => app.move_focus(-1),
        KeyCode::Right | KeyCode::Char('l') => app.move_focus(1),
        _ => {}
    }
    Ok(None)
}

/// Slider position (0..=1) for a mouse row on a vertical slider: the top
/// row is 1, the bottom row 0.
fn slider_fraction(bar: Rect, row: u16) -> f64 {
    let span = bar.height.saturating_sub(1).max(1) as i32;
    let from_bottom = (bar.bottom() as i32 - 1 - row as i32).clamp(0, span);
    from_bottom as f64 / span as f64
}

fn hit(r: &Rect, col: u16, row: u16) -> bool {
    r.contains(Position::new(col, row))
}

fn handle_mouse(app: &mut App, m: MouseEvent) -> Result<Option<Request>, Flow> {
    let quit = app.confirm.is_none() && app.hits.quit.is_some_and(|r| hit(&r, m.column, m.row));
    if quit && m.kind == MouseEventKind::Down(MouseButton::Left) {
        return Err(Flow::Quit);
    }
    Ok(mouse_request(app, m))
}

/// What a mouse event other than clicking quit does.
fn mouse_request(app: &mut App, m: MouseEvent) -> Option<Request> {
    let (col, row) = (m.column, m.row);
    if app.confirm.is_some() {
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            if app.hits.keep.is_some_and(|r| hit(&r, col, row)) {
                return Some(app.keep());
            } else if app.hits.revert.is_some_and(|r| hit(&r, col, row)) {
                return app.revert();
            }
        }
        return None;
    }
    if m.kind != MouseEventKind::Moved {
        app.end_typing();
    }
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Monitors drawn last are on top, so search in reverse.
            if let Some(&(i, _)) = app.hits.monitors.iter().rev().find(|(_, r)| hit(r, col, row)) {
                app.begin_drag(i, (col, row));
            } else if let Some(&(s, _)) = app.hits.section_rows.iter().find(|(_, r)| hit(r, col, row)) {
                app.set_section(s);
                app.focus = Focus::Sections;
            } else if let Some(&(i, _)) = app.hits.list_rows.iter().find(|(_, r)| hit(r, col, row)) {
                app.select(i);
                app.focus = Focus::Displays;
            } else if let Some(&(setting, _)) = app.hits.setting_rows.iter().find(|(_, r)| hit(r, col, row)) {
                app.focus = Focus::Settings;
                app.setting = setting;
            } else if let Some(bar) = app.hits.slider.filter(|r| hit(r, col, row)) {
                app.focus = Focus::Options;
                app.slider_drag = true;
                app.set_slider(slider_fraction(bar, row));
            } else if let Some(&(column, value, _)) = app.hits.times.iter().find(|(_, _, r)| hit(r, col, row)) {
                app.focus = Focus::Options;
                app.time_column = column;
                app.set_time_part(column, value);
            } else if let Some(&(i, _)) = app.hits.choices.iter().find(|(_, r)| hit(r, col, row)) {
                app.focus = Focus::Options;
                app.choose(i);
            } else if app.hits.apply.is_some_and(|r| hit(&r, col, row)) {
                return app.apply();
            } else if app.hits.reset.is_some_and(|r| hit(&r, col, row)) {
                app.reset();
            } else if app.hits.identify.is_some_and(|r| hit(&r, col, row)) {
                return Some(app.identify());
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if app.slider_drag => {
            if let Some(bar) = app.hits.slider {
                app.set_slider(slider_fraction(bar, row));
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => app.drag_to((col, row)),
        MouseEventKind::Up(MouseButton::Left) => {
            app.slider_drag = false;
            app.end_drag();
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            // The wheel does what up/down do in the pane under the mouse.
            if let Some(&(pane, _)) = app.hits.panes.iter().find(|(_, r)| hit(r, col, row)) {
                app.focus = pane;
                app.move_in_focus(if m.kind == MouseEventKind::ScrollUp { -1 } else { 1 });
            }
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use rsds_core::{Mode, Output, Transform};
    use crate::app::{Section, Setting};

    fn output(name: &str, make: &str, modes: &[(i32, i32, i32)], cur: usize, pos: (i32, i32)) -> Output {
        Output {
            name: name.into(),
            make: make.into(),
            model: "Model".into(),
            serial: Some("1".into()),
            physical_size_mm: None,
            modes: modes
                .iter()
                .enumerate()
                .map(|(i, &(w, h, r))| Mode { width: w, height: h, refresh_mhz: r, preferred: i == 0 })
                .collect(),
            enabled: true,
            current_mode: Some(cur),
            position: pos,
            transform: Transform::Normal,
            scale: 1.0,
        }
    }

    pub fn sample() -> Vec<Output> {
        vec![
            output("DP-5", "AOC", &[(3840, 2160, 59996), (1920, 2160, 59988)], 1, (3840, 0)),
            output("DP-3", "ASUS", &[(1920, 1080, 60000), (1920, 1080, 143992)], 0, (1920, 0)),
            output("eDP-1", "Chimei", &[(1920, 1080, 60008)], 0, (0, 0)),
        ]
    }

    fn render(app: &mut App) -> String {
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| ui::draw(f, app)).unwrap();
        let buf = t.backend().buffer();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn renders() {
        let mut app = App::new(&sample(), None);
        println!("{}", render(&mut app));
        assert_eq!(app.entries[0].live.name, "eDP-1");
    }

    #[test]
    fn drag_rightmost_to_left() {
        let mut app = App::new(&sample(), None);
        render(&mut app);
        let (i, r) = app.hits.monitors.iter().find(|(i, _)| app.entries[*i].live.name == "DP-5").copied().unwrap();
        // Grab the middle and drag to the left edge of the terminal.
        let start = (r.x + r.width / 2, r.y + r.height / 2);
        app.begin_drag(i, start);
        app.drag_to((0, start.1));
        app.end_drag();
        let pos: Vec<_> = app.entries.iter().map(|e| (e.live.name.clone(), e.pending.position)).collect();
        println!("{pos:?}\n{}", render(&mut app));
        assert_eq!(app.entries[2].pending.position, (0, 0));
        assert_eq!(app.entries[0].pending.position, (1920, 0));
        assert_eq!(app.entries[1].pending.position, (3840, 0));
    }

    #[test]
    fn disabling_middle_closes_gap() {
        let mut app = App::new(&sample(), None);
        app.select(1); // DP-3
        app.toggle_enabled();
        assert_eq!(app.entries[2].pending.position, (1920, 0));
        assert!(app.is_dirty());
    }

    #[test]
    fn monitor_change_drops_unapplied_edits() {
        let mut app = App::new(&sample(), None);
        app.select(1);
        app.toggle_enabled();
        assert!(app.is_dirty());
        // Same monitors, changed by someone else: edits stay.
        assert!(app.outputs_changed(&sample(), false).is_none());
        assert!(app.is_dirty());
        // A monitor unplugged: edits go, and the UI shows the live state.
        let unplugged: Vec<_> = sample().into_iter().filter(|o| o.name != "DP-5").collect();
        assert!(app.outputs_changed(&unplugged, false).is_none());
        assert!(!app.is_dirty());
        assert_eq!(app.entries.len(), 2);
    }

    #[test]
    fn lid_closing_counts_as_monitor_change() {
        let mut app = App::new(&sample(), None);
        app.select(1);
        app.toggle_enabled();
        app.outputs_changed(&sample(), true);
        assert!(!app.is_dirty());
    }

    /// Choose option `choice` of `setting` for entry `index`.
    fn set(app: &mut App, index: usize, setting: Setting, choice: usize) {
        app.select(index);
        app.setting = setting;
        app.choose(choice);
    }

    /// The live outputs after applying the app's pending configuration.
    fn applied(app: &App) -> Vec<Output> {
        app.entries.iter().map(|e| e.live.with_config(&e.pending)).collect()
    }

    #[test]
    fn duplicate_sits_on_source_and_is_drawn_with_it() {
        let mut app = App::new(&sample(), None);
        // DP-5 (3, 1920x2160) duplicates eDP-1 (1): choices are Off, 1, 2.
        set(&mut app, 2, Setting::Duplicate, 1);
        let dup = &app.entries[2];
        assert_eq!(dup.duplicates.as_deref(), Some("eDP-1"));
        assert_eq!(dup.pending.position, (0, 0));
        assert_eq!(dup.pending.scale, 1.0);
        assert_eq!(app.entries[1].pending.position, (1920, 0));
        let screen = render(&mut app);
        assert!(screen.contains("1 | 3") && screen.contains("eDP-1 | DP-5"), "{screen}");
        assert!(screen.contains("dup of 1"), "{screen}");

        // Once applied, the live state reads back the same way.
        let live = applied(&app);
        app.sync(&live);
        app.reset();
        assert_eq!(app.entries[2].duplicates.as_deref(), Some("eDP-1"));
        assert!(!app.is_dirty());

        // Off: back to a monitor of its own, right of the others.
        set(&mut app, 2, Setting::Duplicate, 0);
        assert_eq!(app.entries[2].duplicates, None);
        assert_eq!(app.entries[2].pending.position, (3840, 0));
    }

    #[test]
    fn duplicate_follows_source_resize_and_drag() {
        let mut app = App::new(&sample(), None);
        // DP-3 (2, 1920x1080) duplicates DP-5 (3, 1920x2160).
        set(&mut app, 1, Setting::Duplicate, 2);
        let source = app.entries[2].rect().unwrap();
        assert_eq!(app.entries[1].pending.position, (source.x, source.y));
        assert_eq!(app.entries[1].pending.scale, 0.5, "shows the whole height");
        // DP-5 to 3840x2160: DP-3 still covers it.
        set(&mut app, 2, Setting::Resolution, 1);
        let source = app.entries[2].rect().unwrap();
        assert_eq!(source.w, 3840);
        assert_eq!(app.entries[1].pending.scale, 0.5);
        assert_eq!(app.entries[1].pending.position, (source.x, source.y));
        // The duplicate's scale can't be chosen.
        app.setting = Setting::Scale;
        app.select(1);
        assert!(app.choices().0.is_empty());
    }

    #[test]
    fn same_size_duplicate_keeps_direction() {
        let mut app = App::new(&sample(), None);
        // eDP-1 (1) duplicates DP-3 (2): both 1920x1080.
        set(&mut app, 0, Setting::Duplicate, 1);
        let live = applied(&app);
        app.sync(&live);
        app.reset();
        assert_eq!(app.entries[0].duplicates.as_deref(), Some("DP-3"));
        assert_eq!(app.entries[1].duplicates, None);
    }

    #[test]
    fn turning_source_off_frees_duplicate() {
        let mut app = App::new(&sample(), None);
        set(&mut app, 2, Setting::Duplicate, 1);
        set(&mut app, 0, Setting::Enabled, 1);
        assert_eq!(app.entries[2].duplicates, None);
        let rects: Vec<_> = app.entries.iter().filter_map(|e| e.rect()).collect();
        assert!(rsds_core::layout::is_valid(&rects), "{rects:?}");
    }

    fn positions(app: &App) -> Vec<(i32, i32)> {
        app.entries.iter().map(|e| e.pending.position).collect()
    }

    #[test]
    fn turned_back_on_returns_to_its_place() {
        let mut app = App::new(&sample(), None);
        let original = positions(&app);
        app.select(1); // DP-3, in the middle
        app.toggle_enabled();
        app.toggle_enabled();
        assert_eq!(positions(&app), original);
        assert!(!app.is_dirty());
    }

    #[test]
    fn unduplicated_returns_to_its_place() {
        let mut app = App::new(&sample(), None);
        let original = positions(&app);
        // DP-3 (1) duplicates eDP-1 (1), then doesn't.
        set(&mut app, 1, Setting::Duplicate, 1);
        assert_eq!(app.entries[1].duplicates.as_deref(), Some("eDP-1"));
        set(&mut app, 1, Setting::Duplicate, 0);
        assert_eq!(app.entries[1].duplicates, None);
        assert_eq!(positions(&app), original);
        assert!(!app.is_dirty());
    }

    #[test]
    fn duplicate_turned_off_and_on_returns_to_its_place() {
        let mut app = App::new(&sample(), None);
        let original = positions(&app);
        set(&mut app, 1, Setting::Duplicate, 1);
        app.toggle_enabled();
        app.toggle_enabled();
        assert_eq!(positions(&app), original);
        assert!(!app.is_dirty());
    }

    #[test]
    fn place_is_forgotten_when_others_change() {
        let mut app = App::new(&sample(), None);
        app.select(1);
        app.toggle_enabled();
        // DP-5 becomes 3840x2160, so DP-3's old place is gone.
        set(&mut app, 2, Setting::Resolution, 1);
        app.select(1);
        app.toggle_enabled();
        // Right of DP-5, centred on it.
        assert_eq!(app.entries[2].pending.position, (1920, 0));
        assert_eq!(app.entries[1].pending.position, (5760, 540));
    }

    /// Apply the pending configuration, as the display server would, and
    /// keep it.
    fn apply_and_keep(app: &mut App) {
        app.apply().unwrap();
        let live = applied(app);
        app.sync(&live);
        app.adopt_live();
        app.keep();
    }

    #[test]
    fn kept_off_still_returns_to_its_place() {
        let mut app = App::new(&sample(), None);
        let original = positions(&app);
        app.select(1);
        app.toggle_enabled();
        apply_and_keep(&mut app);
        assert!(!app.entries[1].live.enabled);
        app.toggle_enabled();
        assert_eq!(positions(&app), original);
        // Discarding that brings the place back for next time.
        app.reset();
        app.toggle_enabled();
        assert_eq!(positions(&app), original);
    }

    #[test]
    fn kept_duplicate_still_returns_to_its_place() {
        let mut app = App::new(&sample(), None);
        let original = positions(&app);
        set(&mut app, 1, Setting::Duplicate, 1);
        apply_and_keep(&mut app);
        set(&mut app, 1, Setting::Duplicate, 0);
        assert_eq!(app.entries[1].duplicates, None);
        assert_eq!(positions(&app), original);
    }

    #[test]
    fn kept_place_is_forgotten_when_others_change() {
        let mut app = App::new(&sample(), None);
        app.select(1);
        app.toggle_enabled();
        apply_and_keep(&mut app);
        set(&mut app, 2, Setting::Resolution, 1);
        apply_and_keep(&mut app);
        app.select(1);
        app.toggle_enabled();
        assert_eq!(app.entries[1].pending.position, (5760, 540));
    }

    #[test]
    fn place_comes_back_with_the_monitors() {
        let mut app = App::new(&sample(), None);
        let original = positions(&app);
        app.select(1);
        app.toggle_enabled();
        apply_and_keep(&mut app);
        let live = applied(&app);
        let mut profiles = Profiles::default();
        profiles.remember(&live, &app.duplicate_hint(&live), app.restores_for(&live));

        // DP-5 unplugged and plugged back in.
        let unplugged: Vec<_> = live.iter().filter(|o| o.name != "DP-5").cloned().collect();
        app.outputs_changed(&unplugged, false);
        app.outputs_changed(&live, false);
        app.use_profile(profiles.find(&live).unwrap());
        app.select(1);
        app.toggle_enabled();
        assert_eq!(positions(&app), original);
    }

    #[test]
    fn lid_close_info_follows_choice() {
        let mut app = App::new(&sample(), None);
        app.set_section(Section::LidClose);
        app.focus = Focus::Options;
        app.lid_close.lid_close = rsds_core::lid_close::LidClose::Never;
        let screen = render(&mut app);
        println!("{screen}");
        assert!(screen.contains("never suspends"), "{screen}");
        assert!(!screen.contains("left to the system"), "{screen}");
    }

    /// Entry `index`'s box in the arrangement.
    fn box_of(app: &mut App, index: usize) -> ratatui::layout::Rect {
        render(app);
        app.hits.monitors.iter().find(|(i, _)| *i == index).unwrap().1
    }

    /// Drag entry `index` by its middle to cell `to`.
    fn drag(app: &mut App, index: usize, to: (u16, u16)) {
        let r = box_of(app, index);
        app.begin_drag(index, (r.x + r.width / 2, r.y + r.height / 2));
        app.drag_to(to);
        app.end_drag();
    }

    #[test]
    fn dropping_on_a_monitor_duplicates_it() {
        let mut app = App::new(&sample(), None);
        let original = positions(&app);
        // DP-3 (1) dropped on eDP-1 (0).
        let r = box_of(&mut app, 0);
        drag(&mut app, 1, (r.x + r.width / 2, r.y + r.height / 2));
        assert_eq!(app.entries[1].duplicates.as_deref(), Some("eDP-1"));
        assert_eq!(app.entries[1].pending.position, (0, 0));
        // DP-5 closes the gap.
        assert_eq!(app.entries[2].pending.position, (1920, 0));
        // Stopping duplicating puts it back where it was.
        set(&mut app, 1, Setting::Duplicate, 0);
        assert_eq!(positions(&app), original);
    }

    #[test]
    fn dropping_on_an_edge_snaps_alongside() {
        let mut app = App::new(&sample(), None);
        // DP-3 (1) dropped over eDP-1's (0) left edge.
        let r = box_of(&mut app, 0);
        drag(&mut app, 1, (r.x, r.y + r.height / 2));
        assert_eq!(app.entries[1].duplicates, None);
        let rects: Vec<_> = app.entries.iter().filter_map(|e| e.rect()).collect();
        assert!(rsds_core::layout::is_valid(&rects), "{rects:?}");
    }
}
