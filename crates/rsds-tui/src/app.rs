//! UI state and the operations the user can perform on it.

use std::time::{Duration, Instant};

use rsds_core::{
    Mode, Output, OutputConfig, Transform,
    layout::{self, Rect},
};

/// How long a newly applied configuration waits for confirmation.
pub const REVERT_AFTER: Duration = Duration::from_secs(15);

/// How long Identify shows the monitor numbers.
pub const IDENTIFY_FOR: Duration = Duration::from_secs(3);

/// A monitor in the UI: its live state and the edits not yet applied.
pub struct Entry {
    /// Number shown in the UI (and later by Identify). Stable while running.
    pub number: usize,
    pub live: Output,
    pub pending: OutputConfig,
}

impl Entry {
    fn new(number: usize, live: Output) -> Self {
        let pending = live.config();
        Self { number, live, pending }
    }

    /// Rectangle in the global layout according to the pending config.
    pub fn rect(&self) -> Option<Rect> {
        self.pending.rect()
    }

    pub fn is_dirty(&self) -> bool {
        let live = self.live.config();
        if !self.pending.enabled && !live.enabled {
            return false;
        }
        self.pending != live
    }

    /// Distinct resolutions, smallest first.
    pub fn resolutions(&self) -> Vec<(i32, i32)> {
        let mut res: Vec<(i32, i32)> = Vec::new();
        for m in &self.live.modes {
            if !res.contains(&(m.width, m.height)) {
                res.push((m.width, m.height));
            }
        }
        res.sort_by_key(|&(w, h)| (w as i64 * h as i64, w));
        res
    }

    /// Modes with the given resolution, distinct refresh rates, slowest first.
    pub fn refresh_modes(&self, (w, h): (i32, i32)) -> Vec<Mode> {
        let mut modes: Vec<Mode> = Vec::new();
        for m in self.live.modes.iter().filter(|m| m.width == w && m.height == h) {
            if !modes.iter().any(|x| x.refresh_mhz == m.refresh_mhz) {
                modes.push(*m);
            }
        }
        modes.sort_by_key(|m| m.refresh_mhz);
        modes
    }

    /// Whether the given resolution is the monitor's preferred one.
    pub fn is_preferred_resolution(&self, (w, h): (i32, i32)) -> bool {
        self.live
            .preferred_mode()
            .is_some_and(|m| m.width == w && m.height == h)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Displays,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Enabled,
    Resolution,
    Refresh,
    Orientation,
    Scale,
}

pub const SETTINGS: [Setting; 5] = [
    Setting::Enabled,
    Setting::Resolution,
    Setting::Refresh,
    Setting::Orientation,
    Setting::Scale,
];

pub const ORIENTATIONS: [Transform; 4] = [
    Transform::Normal,
    Transform::Rotate90,
    Transform::Rotate180,
    Transform::Rotate270,
];

pub fn orientation_label(t: Transform) -> &'static str {
    match t {
        Transform::Normal => "Landscape",
        Transform::Rotate90 => "Portrait (left)",
        Transform::Rotate180 => "Landscape (flipped)",
        Transform::Rotate270 => "Portrait (right)",
        Transform::Flipped => "Mirrored",
        Transform::Flipped90 => "Mirrored, left",
        Transform::Flipped180 => "Mirrored, flipped",
        Transform::Flipped270 => "Mirrored, right",
    }
}

/// One possible value of a setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choice {
    Enabled(bool),
    Resolution((i32, i32)),
    Refresh(Mode),
    Orientation(Transform),
    Scale(f64),
}

impl Choice {
    fn is_current(&self, e: &Entry) -> bool {
        let mode = e.pending.mode;
        match *self {
            Choice::Enabled(on) => on == e.pending.enabled,
            Choice::Resolution((w, h)) => mode.is_some_and(|m| m.width == w && m.height == h),
            Choice::Refresh(r) => mode.is_some_and(|m| m.same_as(&r)),
            Choice::Orientation(t) => t == e.pending.transform,
            Choice::Scale(s) => (s - e.pending.scale).abs() < 1e-6,
        }
    }

    /// Whether this is the monitor's preferred (recommended) value.
    pub fn is_recommended(&self, e: &Entry) -> bool {
        match *self {
            Choice::Resolution(res) => e.is_preferred_resolution(res),
            Choice::Refresh(m) => m.preferred,
            _ => false,
        }
    }

    /// Label for the value; `short` marks recommended values with "*".
    pub fn label(&self, e: &Entry, short: bool) -> String {
        let rec = match (self.is_recommended(e), short) {
            (false, _) => "",
            (true, true) => " *",
            (true, false) => " (recommended)",
        };
        match *self {
            Choice::Enabled(on) => if on { "On" } else { "Off" }.into(),
            Choice::Resolution((w, h)) => format!("{w} × {h}{rec}"),
            Choice::Refresh(m) => format!("{:.3} Hz{rec}", m.refresh_hz()),
            Choice::Orientation(t) => orientation_label(t).into(),
            Choice::Scale(s) => format!("{}%", (s * 100.0).round()),
        }
    }
}

const SCALES: [f64; 9] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0];

/// Mapping between layout pixels and terminal cells for the arrangement view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub area: ratatui::layout::Rect,
    /// Layout pixel shown at the top-left cell of `area`.
    pub origin: (f64, f64),
    /// Layout pixels per terminal column. A row is twice as tall.
    pub px_per_col: f64,
}

impl View {
    const ROW_ASPECT: f64 = 2.0;

    /// Fit `rects` into `area`, centred, with room around them to drag a
    /// monitor past either end of the arrangement.
    pub fn fit(area: ratatui::layout::Rect, rects: &[Rect]) -> Self {
        let pad_x = rects.iter().map(|r| r.w).max().unwrap_or(1920) as f64 / 2.0;
        let pad_y = rects.iter().map(|r| r.h).max().unwrap_or(1080) as f64 / 2.0;
        let min_x = rects.iter().map(|r| r.x).min().unwrap_or(0) as f64 - pad_x;
        let min_y = rects.iter().map(|r| r.y).min().unwrap_or(0) as f64 - pad_y;
        let max_x = rects.iter().map(|r| r.right()).max().unwrap_or(1920) as f64 + pad_x;
        let max_y = rects.iter().map(|r| r.bottom()).max().unwrap_or(1080) as f64 + pad_y;
        let (bw, bh) = ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
        let cols = (area.width as f64).max(1.0);
        let rows = (area.height as f64).max(1.0);
        let px_per_col = (bw / cols).max(bh / (rows * Self::ROW_ASPECT));
        let used_cols = bw / px_per_col;
        let used_rows = bh / (px_per_col * Self::ROW_ASPECT);
        let pad_cols = (area.width as f64 - used_cols) / 2.0;
        let pad_rows = (area.height as f64 - used_rows) / 2.0;
        Self {
            area,
            origin: (
                min_x - pad_cols * px_per_col,
                min_y - pad_rows * px_per_col * Self::ROW_ASPECT,
            ),
            px_per_col,
        }
    }

    /// Terminal cell rectangle for a layout rect, clipped to the view area.
    pub fn to_cells(self, r: Rect) -> ratatui::layout::Rect {
        let x0 = ((r.x as f64 - self.origin.0) / self.px_per_col).round();
        let y0 = ((r.y as f64 - self.origin.1) / (self.px_per_col * Self::ROW_ASPECT)).round();
        let x1 = ((r.right() as f64 - self.origin.0) / self.px_per_col).round();
        let y1 = ((r.bottom() as f64 - self.origin.1) / (self.px_per_col * Self::ROW_ASPECT)).round();
        let a = self.area;
        let clamp_x = |v: f64| (v.max(0.0) as u16).min(a.width) + a.x;
        let clamp_y = |v: f64| (v.max(0.0) as u16).min(a.height) + a.y;
        let (cx0, cy0, cx1, cy1) = (clamp_x(x0), clamp_y(y0), clamp_x(x1), clamp_y(y1));
        ratatui::layout::Rect::new(cx0, cy0, cx1 - cx0, cy1 - cy0)
    }

    /// Layout pixel offset for a movement of (dcol, drow) cells.
    pub fn cells_to_px(self, dcol: i32, drow: i32) -> (i32, i32) {
        (
            (dcol as f64 * self.px_per_col).round() as i32,
            (drow as f64 * self.px_per_col * Self::ROW_ASPECT).round() as i32,
        )
    }
}

pub struct Drag {
    pub index: usize,
    pub start_cell: (u16, u16),
    pub start_pos: (i32, i32),
    pub moved: bool,
}

pub struct Confirm {
    pub deadline: Instant,
    pub previous: Vec<OutputConfig>,
}

/// Something the event loop must do on the app's behalf.
pub enum Request {
    /// Apply a user's edits; needs confirmation afterwards.
    Apply(Vec<OutputConfig>),
    /// Restore the configuration from before the last apply.
    Revert(Vec<OutputConfig>),
    /// Show each monitor's number on it: (connector name, number).
    Identify(Vec<(String, usize)>),
    /// Remove the Identify badges.
    EndIdentify,
    /// Save the live configuration as the profile for this set of monitors.
    SaveProfile,
}

/// Screen regions recorded during drawing, for mouse hit testing.
#[derive(Default)]
pub struct Hits {
    pub monitors: Vec<(usize, ratatui::layout::Rect)>,
    pub list_rows: Vec<(usize, ratatui::layout::Rect)>,
    /// (setting index, whole row, decrease arrow, increase arrow)
    pub setting_rows: Vec<(usize, ratatui::layout::Rect, ratatui::layout::Rect, ratatui::layout::Rect)>,
    /// (choice index, cell) in the options panel.
    pub choices: Vec<(usize, ratatui::layout::Rect)>,
    pub apply: Option<ratatui::layout::Rect>,
    pub reset: Option<ratatui::layout::Rect>,
    pub keep: Option<ratatui::layout::Rect>,
    pub revert: Option<ratatui::layout::Rect>,
    pub identify: Option<ratatui::layout::Rect>,
}

pub struct App {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub focus: Focus,
    pub setting: usize,
    pub drag: Option<Drag>,
    /// View used while dragging, so the map does not rescale under the mouse.
    pub frozen_view: Option<View>,
    pub view: Option<View>,
    pub confirm: Option<Confirm>,
    /// When the Identify badges should be removed.
    pub identify_until: Option<Instant>,
    pub status: Option<String>,
    pub hits: Hits,
}

impl App {
    pub fn new(outputs: &[Output]) -> Self {
        let mut sorted: Vec<&Output> = outputs.iter().collect();
        sorted.sort_by(|a, b| {
            (!a.is_internal(), natural_key(&a.name)).cmp(&(!b.is_internal(), natural_key(&b.name)))
        });
        let entries = sorted
            .into_iter()
            .enumerate()
            .map(|(i, o)| Entry::new(i + 1, o.clone()))
            .collect();
        Self {
            entries,
            selected: 0,
            focus: Focus::Displays,
            setting: 0,
            drag: None,
            frozen_view: None,
            view: None,
            confirm: None,
            identify_until: None,
            status: None,
            hits: Hits::default(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.entries.iter().any(Entry::is_dirty)
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// The display server reported new state (after apply, or hotplug).
    pub fn sync(&mut self, outputs: &[Output]) {
        let mut next_number = self.entries.iter().map(|e| e.number).max().unwrap_or(0) + 1;
        self.entries.retain(|e| outputs.iter().any(|o| o.name == e.live.name));
        for o in outputs {
            match self.entries.iter_mut().find(|e| e.live.name == o.name) {
                Some(e) => {
                    let was_clean = !e.is_dirty();
                    e.live = o.clone();
                    if was_clean {
                        e.pending = o.config();
                    }
                }
                None => {
                    self.entries.push(Entry::new(next_number, o.clone()));
                    next_number += 1;
                }
            }
        }
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
        if self.drag.as_ref().is_some_and(|d| d.index >= self.entries.len()) {
            self.drag = None;
            self.frozen_view = None;
        }
    }

    pub fn select(&mut self, index: usize) {
        if index < self.entries.len() {
            self.selected = index;
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        let n = self.entries.len() as i32;
        if n > 0 {
            self.selected = (self.selected as i32 + delta).rem_euclid(n) as usize;
        }
    }

    pub fn move_setting(&mut self, delta: i32) {
        let n = SETTINGS.len() as i32;
        self.setting = (self.setting as i32 + delta).rem_euclid(n) as usize;
    }

    /// The available values of the current setting for the selected
    /// monitor, in display order, and the index of the pending value.
    pub fn choices(&self) -> (Vec<Choice>, Option<usize>) {
        let Some(e) = self.selected_entry() else { return (Vec::new(), None) };
        let setting = SETTINGS[self.setting];
        if !e.pending.enabled && setting != Setting::Enabled {
            return (Vec::new(), None);
        }
        let choices: Vec<Choice> = match setting {
            Setting::Enabled => vec![Choice::Enabled(true), Choice::Enabled(false)],
            Setting::Resolution => e.resolutions().into_iter().map(Choice::Resolution).collect(),
            Setting::Refresh => match e.pending.mode {
                Some(m) => e.refresh_modes((m.width, m.height)).into_iter().map(Choice::Refresh).collect(),
                None => Vec::new(),
            },
            Setting::Orientation => {
                let mut v: Vec<Choice> = ORIENTATIONS.iter().map(|t| Choice::Orientation(*t)).collect();
                if !ORIENTATIONS.contains(&e.pending.transform) {
                    v.push(Choice::Orientation(e.pending.transform));
                }
                v
            }
            Setting::Scale => {
                let mut scales = SCALES.to_vec();
                if !scales.iter().any(|s| (s - e.pending.scale).abs() < 1e-6) {
                    scales.push(e.pending.scale);
                    scales.sort_by(f64::total_cmp);
                }
                scales.into_iter().map(Choice::Scale).collect()
            }
        };
        let current = choices.iter().position(|c| c.is_current(e));
        (choices, current)
    }

    /// Step the current setting of the selected monitor through its choices.
    pub fn change_setting(&mut self, delta: i32) {
        if SETTINGS[self.setting] == Setting::Enabled {
            self.status = None;
            self.toggle_enabled();
            return;
        }
        let (choices, current) = self.choices();
        if choices.is_empty() {
            return;
        }
        let next = match current {
            Some(i) => step(i, delta, choices.len()),
            None => 0,
        };
        self.choose(next);
    }

    /// Set the current setting of the selected monitor to choice `index`.
    pub fn choose(&mut self, index: usize) {
        self.status = None;
        let (choices, _) = self.choices();
        let Some(&choice) = choices.get(index) else { return };
        let i = self.selected;
        let e = &mut self.entries[i];
        match choice {
            Choice::Enabled(on) => {
                if on != e.pending.enabled {
                    self.toggle_enabled();
                }
                return;
            }
            Choice::Resolution(res) => {
                let modes = e.refresh_modes(res);
                let keep_refresh = e.pending.mode.map(|m| m.refresh_mhz);
                // Keep the refresh rate if this resolution has (nearly) the
                // same one, preferring the monitor's preferred mode.
                let close = keep_refresh.and_then(|r| {
                    modes
                        .iter()
                        .filter(|m| (m.refresh_mhz - r).abs() < 500)
                        .min_by_key(|m| (!m.preferred, (m.refresh_mhz - r).abs()))
                });
                e.pending.mode = close
                    .or_else(|| modes.iter().find(|m| m.preferred))
                    .or(modes.last())
                    .copied();
            }
            Choice::Refresh(mode) => {
                e.pending.mode = Some(mode);
                return; // size unchanged
            }
            Choice::Orientation(t) => e.pending.transform = t,
            Choice::Scale(scale) => e.pending.scale = scale,
        }
        self.fixup_layout(i);
    }

    pub fn toggle_enabled(&mut self) {
        let i = self.selected;
        let enabled_count = self.entries.iter().filter(|e| e.pending.enabled).count();
        let Some(e) = self.entries.get_mut(i) else { return };
        if e.pending.enabled {
            if enabled_count <= 1 {
                self.status = Some("Can't turn off the only active display".into());
                return;
            }
            e.pending.enabled = false;
        } else {
            e.pending.enabled = true;
            if e.pending.mode.is_none() {
                e.pending.mode = e.live.preferred_mode().copied();
            }
            if e.pending.scale <= 0.0 {
                e.pending.scale = 1.0;
            }
            // Turned-on monitors go to the right of the others.
            let others: Vec<Rect> = self.other_rects(i);
            let x = others.iter().map(Rect::right).max().unwrap_or(0);
            let y = others.iter().map(|r| r.y).min().unwrap_or(0);
            self.entries[i].pending.position = (x, y);
        }
        self.fixup_layout(i);
    }

    /// Enabled monitor rects other than `index`.
    fn other_rects(&self, index: usize) -> Vec<Rect> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != index)
            .filter_map(|(_, e)| e.rect())
            .collect()
    }

    /// After monitor `changed` changed size or was turned on/off, move the
    /// others so nothing overlaps and nothing floats detached.
    fn fixup_layout(&mut self, changed: usize) {
        let anchor = self.entries[changed].rect();
        let centre = anchor
            .map(|r| (r.x + r.w / 2, r.y + r.h / 2))
            .unwrap_or(self.entries[changed].pending.position);
        let (indices, rects): (Vec<usize>, Vec<Rect>) = self
            .entries
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != changed)
            .filter_map(|(j, e)| e.rect().map(|r| (j, r)))
            .unzip();
        for (j, r) in indices.into_iter().zip(layout::compact(anchor, centre, &rects)) {
            self.entries[j].pending.position = (r.x, r.y);
        }
        self.normalize();
    }

    /// Shift all enabled monitors so the layout starts at (0, 0).
    fn normalize(&mut self) {
        let rects: Vec<Rect> = self.entries.iter().filter_map(Entry::rect).collect();
        let (dx, dy) = layout::normalize_offset(&rects);
        for e in self.entries.iter_mut().filter(|e| e.pending.enabled) {
            e.pending.position.0 += dx;
            e.pending.position.1 += dy;
        }
    }

    pub fn begin_drag(&mut self, index: usize, cell: (u16, u16)) {
        self.select(index);
        self.focus = Focus::Displays;
        let Some(e) = self.entries.get(index) else { return };
        if !e.pending.enabled {
            return;
        }
        self.drag = Some(Drag {
            index,
            start_cell: cell,
            start_pos: e.pending.position,
            moved: false,
        });
        self.frozen_view = self.view;
    }

    pub fn drag_to(&mut self, cell: (u16, u16)) {
        let (Some(d), Some(view)) = (self.drag.as_mut(), self.frozen_view) else { return };
        let dcol = cell.0 as i32 - d.start_cell.0 as i32;
        let drow = cell.1 as i32 - d.start_cell.1 as i32;
        let (dx, dy) = view.cells_to_px(dcol, drow);
        d.moved |= dcol != 0 || drow != 0;
        self.entries[d.index].pending.position = (d.start_pos.0 + dx, d.start_pos.1 + dy);
    }

    pub fn end_drag(&mut self) {
        let Some(d) = self.drag.take() else { return };
        let view = self.frozen_view.take();
        if !d.moved {
            return;
        }
        let Some(rect) = self.entries[d.index].rect() else { return };
        // Edges within about two cells of another edge snap to it.
        let threshold = view.map_or(0, |v| (v.px_per_col * 2.0) as i32);
        let snapped = layout::snap(rect, &self.other_rects(d.index), threshold);
        self.entries[d.index].pending.position = (snapped.x, snapped.y);
        self.fixup_layout(d.index);
    }

    /// Discard unapplied edits.
    pub fn reset(&mut self) {
        for e in &mut self.entries {
            e.pending = e.live.config();
        }
        self.status = None;
    }

    pub fn apply(&mut self) -> Option<Request> {
        if self.confirm.is_some() || !self.is_dirty() {
            return None;
        }
        Some(Request::Apply(self.entries.iter().map(|e| e.pending.clone()).collect()))
    }

    /// Called after a user-initiated apply succeeded.
    pub fn applied(&mut self, previous: Vec<OutputConfig>) {
        self.confirm = Some(Confirm { deadline: Instant::now() + REVERT_AFTER, previous });
        self.status = None;
    }

    /// The user confirmed the new configuration: remember it.
    pub fn keep(&mut self) -> Request {
        self.confirm = None;
        Request::SaveProfile
    }

    pub fn revert(&mut self) -> Option<Request> {
        let c = self.confirm.take()?;
        for e in &mut self.entries {
            if let Some(p) = c.previous.iter().find(|p| p.name == e.pending.name) {
                e.pending = p.clone();
            }
        }
        Some(Request::Revert(c.previous))
    }

    /// Time until the next timed event, if any.
    pub fn next_deadline(&self) -> Option<Instant> {
        let confirm = self.confirm.as_ref().map(|c| c.deadline);
        match (confirm, self.identify_until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Show each active monitor's number on it for a few seconds.
    pub fn identify(&mut self) -> Request {
        self.identify_until = Some(Instant::now() + IDENTIFY_FOR);
        let labels = self
            .entries
            .iter()
            .filter(|e| e.live.enabled)
            .map(|e| (e.live.name.clone(), e.number))
            .collect();
        Request::Identify(labels)
    }

    pub fn tick(&mut self, now: Instant) -> Option<Request> {
        if self.identify_until.is_some_and(|t| now >= t) {
            self.identify_until = None;
            return Some(Request::EndIdentify);
        }
        if self.confirm.as_ref().is_some_and(|c| now >= c.deadline) {
            return self.revert();
        }
        None
    }
}

/// Move an index by `delta`, clamped to `0..len`.
fn step(idx: usize, delta: i32, len: usize) -> usize {
    (idx as i32 + delta).clamp(0, len.saturating_sub(1) as i32) as usize
}

/// Sort key that orders "DP-10" after "DP-9".
fn natural_key(s: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut num: Option<u64> = None;
    for c in s.chars() {
        if let Some(d) = c.to_digit(10) {
            num = Some(num.unwrap_or(0) * 10 + d as u64);
        } else {
            if let Some(n) = num.take() {
                out.push((std::mem::take(&mut text), n));
            }
            text.push(c);
        }
    }
    out.push((text, num.unwrap_or(0)));
    out
}
