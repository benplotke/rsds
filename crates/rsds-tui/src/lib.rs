//! Terminal UI for arranging and configuring monitors.

mod app;
mod ui;

use std::{
    io,
    time::{Duration, Instant},
};

use app::{App, Focus, Request, SETTINGS, Setting};
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
use rsds_core::{Backend, OutputConfig};
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
    let mut app = App::new(backend.outputs());
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
            app.sync(backend.outputs());
        }
        while event::poll(Duration::ZERO)? {
            let request = match event::read()? {
                Event::Key(key) => match handle_key(&mut app, key) {
                    Ok(r) => r,
                    Err(Flow::Quit) => return quit(&mut app, backend),
                    Err(Flow::Continue) => None,
                },
                Event::Mouse(mouse) => handle_mouse(&mut app, mouse),
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
            app.reset();
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
                app.keep();
                None
            }
            KeyCode::Esc | KeyCode::Char('n') => app.revert(),
            KeyCode::Char('q') => return Err(Flow::Quit),
            _ => None,
        });
    }
    match key.code {
        KeyCode::Char('q') => return Err(Flow::Quit),
        KeyCode::Char('a') => return Ok(app.apply()),
        KeyCode::Char('r') => app.reset(),
        KeyCode::Esc => app.focus = Focus::Displays,
        KeyCode::Tab | KeyCode::BackTab => {
            app.focus = match app.focus {
                Focus::Displays => Focus::Settings,
                Focus::Settings => Focus::Displays,
            }
        }
        KeyCode::Up | KeyCode::Char('k') => match app.focus {
            Focus::Displays => app.move_selection(-1),
            Focus::Settings => app.move_setting(-1),
        },
        KeyCode::Down | KeyCode::Char('j') => match app.focus {
            Focus::Displays => app.move_selection(1),
            Focus::Settings => app.move_setting(1),
        },
        KeyCode::Left | KeyCode::Char('h') if app.focus == Focus::Settings => app.change_setting(-1),
        KeyCode::Right | KeyCode::Char('l') if app.focus == Focus::Settings => app.change_setting(1),
        KeyCode::Right | KeyCode::Enter if app.focus == Focus::Displays => app.focus = Focus::Settings,
        KeyCode::Char(' ') | KeyCode::Enter
            if app.focus == Focus::Settings && SETTINGS[app.setting] == Setting::Enabled =>
        {
            app.toggle_enabled()
        }
        _ => {}
    }
    Ok(None)
}

fn hit(r: &Rect, col: u16, row: u16) -> bool {
    r.contains(Position::new(col, row))
}

fn handle_mouse(app: &mut App, m: MouseEvent) -> Option<Request> {
    let (col, row) = (m.column, m.row);
    if app.confirm.is_some() {
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            if app.hits.keep.is_some_and(|r| hit(&r, col, row)) {
                app.keep();
            } else if app.hits.revert.is_some_and(|r| hit(&r, col, row)) {
                return app.revert();
            }
        }
        return None;
    }
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Monitors drawn last are on top, so search in reverse.
            if let Some(&(i, _)) = app.hits.monitors.iter().rev().find(|(_, r)| hit(r, col, row)) {
                app.begin_drag(i, (col, row));
            } else if let Some(&(i, _)) = app.hits.list_rows.iter().find(|(_, r)| hit(r, col, row)) {
                app.select(i);
                app.focus = Focus::Displays;
            } else if let Some(&(i, _, dec, inc)) =
                app.hits.setting_rows.iter().find(|(_, r, _, _)| hit(r, col, row))
            {
                app.focus = Focus::Settings;
                app.setting = i;
                if hit(&dec, col, row) {
                    app.change_setting(-1);
                } else if hit(&inc, col, row) || SETTINGS[i] == Setting::Enabled {
                    app.change_setting(1);
                }
            } else if let Some(&(i, _)) = app.hits.choices.iter().find(|(_, r)| hit(r, col, row)) {
                app.choose(i);
            } else if app.hits.apply.is_some_and(|r| hit(&r, col, row)) {
                return app.apply();
            } else if app.hits.reset.is_some_and(|r| hit(&r, col, row)) {
                app.reset();
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => app.drag_to((col, row)),
        MouseEventKind::Up(MouseButton::Left) => app.end_drag(),
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let delta = if m.kind == MouseEventKind::ScrollUp { 1 } else { -1 };
            if let Some(&(i, ..)) = app.hits.setting_rows.iter().find(|(_, r, _, _)| hit(r, col, row)) {
                app.focus = Focus::Settings;
                app.setting = i;
                app.change_setting(delta);
            } else if app.hits.list_rows.iter().any(|(_, r)| hit(r, col, row)) {
                app.move_selection(-delta);
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
        let mut app = App::new(&sample());
        println!("{}", render(&mut app));
        assert_eq!(app.entries[0].live.name, "eDP-1");
    }

    #[test]
    fn drag_rightmost_to_left() {
        let mut app = App::new(&sample());
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
        let mut app = App::new(&sample());
        app.select(1); // DP-3
        app.toggle_enabled();
        assert_eq!(app.entries[2].pending.position, (1920, 0));
        assert!(app.is_dirty());
    }
}
