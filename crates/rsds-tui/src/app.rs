//! UI state and the operations the user can perform on it.

use std::time::{Duration, Instant};

use rsds_core::{
    Mode, Output, OutputConfig, Transform,
    nightlight::{self, Nightlight, TimeOfDay},
    lid_close::{LidClose, LidCloseSettings},
    layout::{self, Rect},
    mirror,
    profile::{Profile, SavedOutput, SavedRestore, active_outputs, duplicate_sources, monitor_set},
    unconfirmed,
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
    /// Connector of the monitor this one duplicates in the pending config.
    /// Its position and scale then follow from that monitor's.
    pub duplicates: Option<String>,
}

impl Entry {
    fn new(number: usize, live: Output) -> Self {
        let pending = live.config();
        Self { number, live, pending, duplicates: None }
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
    /// The left-most pane: Displays / Nightlight / Lid Close.
    Sections,
    /// The monitor list (Displays section only).
    Displays,
    Settings,
    Options,
}

/// What the bottom panes are about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Displays,
    Nightlight,
    LidClose,
}

pub const SECTIONS: [Section; 3] = [Section::Displays, Section::Nightlight, Section::LidClose];

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Displays => "Displays",
            Section::Nightlight => "Nightlight",
            Section::LidClose => "Lid Close",
        }
    }

    /// The panes shown for this section, left to right.
    pub fn panes(self) -> &'static [Focus] {
        match self {
            Section::Displays => &[Focus::Sections, Focus::Displays, Focus::Settings, Focus::Options],
            Section::Nightlight => &[Focus::Sections, Focus::Settings, Focus::Options],
            // A single setting, so its choices are the only other pane.
            Section::LidClose => &[Focus::Sections, Focus::Options],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Enabled,
    /// Built-in panel only; applied immediately, not part of Apply/Revert.
    Brightness,
    Resolution,
    Refresh,
    Orientation,
    Scale,
    /// Show the same as another monitor.
    Duplicate,
    // Nightlight section; applied immediately (saved for the daemon).
    NightlightEnabled,
    Temperature,
    Schedule,
    ScheduleStart,
    ScheduleEnd,
    // Lid Close section; applied immediately (saved for the daemon).
    LidClose,
}

const NIGHTLIGHT_SETTINGS: [Setting; 5] = [
    Setting::NightlightEnabled,
    Setting::Temperature,
    Setting::Schedule,
    Setting::ScheduleStart,
    Setting::ScheduleEnd,
];

const LID_CLOSE_SETTINGS: [Setting; 1] = [Setting::LidClose];

/// Colour temperature step for keys and the scroll wheel, in kelvin.
pub const TEMPERATURE_STEP: u32 = 100;
/// Minute step of the schedule's time picker.
pub const MINUTE_STEP: u16 = 5;

/// Column of the schedule's time picker in the options pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeColumn {
    Hour,
    Minute,
}

impl TimeColumn {
    /// Largest value of this column.
    fn max(self) -> u16 {
        match self {
            TimeColumn::Hour => 23,
            TimeColumn::Minute => 59,
        }
    }
}

/// The hour `steps` hours after `h`, wrapping round the day.
pub fn step_hour(h: u16, steps: i32) -> u16 {
    (h as i32 + steps).rem_euclid(24) as u16
}

/// The minute `steps` [`MINUTE_STEP`]s after `m`, wrapping round the hour.
/// A minute off the step grid first moves to the grid in that direction.
pub fn step_minute(m: u16, steps: i32) -> u16 {
    let (m, s) = (m as i32, MINUTE_STEP as i32);
    let on_grid = match steps.signum() {
        0 => return m as u16,
        1 => m.div_euclid(s) * s,
        _ => (m + s - 1).div_euclid(s) * s,
    };
    (on_grid + steps * s).rem_euclid(60) as u16
}

pub const DISPLAY_SETTINGS: [Setting; 6] = [
    Setting::Enabled,
    Setting::Resolution,
    Setting::Refresh,
    Setting::Orientation,
    Setting::Scale,
    Setting::Duplicate,
];

/// Brightness step for keys and the scroll wheel, in percent.
const BRIGHTNESS_STEP: i32 = 5;

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
    /// Duplicate the monitor with this number, or (None) don't.
    Duplicate(Option<usize>),
    LidClose(LidClose),
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
            // Depend on the other monitors: see `App::choices`.
            Choice::Duplicate(_) | Choice::LidClose(_) => false,
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
            Choice::Duplicate(None) => "Off".into(),
            // Named by `App::choice_label`.
            Choice::Duplicate(Some(n)) => n.to_string(),
            Choice::LidClose(l) => l.label().into(),
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
    /// It includes the cells at both its start and end edges, so rects
    /// that share an edge share a row or column of cells there.
    pub fn to_cells(self, r: Rect) -> ratatui::layout::Rect {
        let x0 = ((r.x as f64 - self.origin.0) / self.px_per_col).round();
        let y0 = ((r.y as f64 - self.origin.1) / (self.px_per_col * Self::ROW_ASPECT)).round();
        let x1 = ((r.right() as f64 - self.origin.0) / self.px_per_col).round();
        let y1 = ((r.bottom() as f64 - self.origin.1) / (self.px_per_col * Self::ROW_ASPECT)).round();
        let a = self.area;
        let clamp_x = |v: f64| (v.max(0.0) as u16).min(a.width) + a.x;
        let clamp_y = |v: f64| (v.max(0.0) as u16).min(a.height) + a.y;
        let (cx0, cy0) = (clamp_x(x0), clamp_y(y0));
        let (cx1, cy1) = (clamp_x(x1 + 1.0), clamp_y(y1 + 1.0));
        ratatui::layout::Rect::new(cx0, cy0, cx1 - cx0, cy1 - cy0)
    }

    /// How close (x, y) in layout pixels an edge must be dropped to another
    /// to snap to it: half a cell, so any alignment can be reached by
    /// dragging, and moving one cell away from one is never undone.
    pub fn snap_threshold(self) -> (i32, i32) {
        let half = self.px_per_col / 2.0;
        (half as i32, (half * Self::ROW_ASPECT) as i32)
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
    /// The bottom panes.
    pub panes: Vec<(Focus, ratatui::layout::Rect)>,
    pub section_rows: Vec<(Section, ratatui::layout::Rect)>,
    pub setting_rows: Vec<(Setting, ratatui::layout::Rect)>,
    /// The brightness slider's bar.
    pub slider: Option<ratatui::layout::Rect>,
    /// (choice index, cell) in the options panel.
    pub choices: Vec<(usize, ratatui::layout::Rect)>,
    /// (column, value, cell) of the times shown in the time picker.
    pub times: Vec<(TimeColumn, u16, ratatui::layout::Rect)>,
    pub apply: Option<ratatui::layout::Rect>,
    pub reset: Option<ratatui::layout::Rect>,
    pub keep: Option<ratatui::layout::Rect>,
    pub revert: Option<ratatui::layout::Rect>,
    pub identify: Option<ratatui::layout::Rect>,
    pub quit: Option<ratatui::layout::Rect>,
}

pub struct App {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub focus: Focus,
    pub section: Section,
    /// Nightlight settings, saved for the daemon on every change.
    pub nightlight: Nightlight,
    /// Lid Close settings, saved for the daemon on every change.
    pub lid_close: LidCloseSettings,
    /// Whether the daemon (which applies Nightlight and Lid Close) is running.
    pub daemon_running: bool,
    pub setting: Setting,
    /// Column of the schedule's time picker that up/down and typing change.
    pub time_column: TimeColumn,
    /// Digits typed so far into the time picker's column.
    typed_digits: u8,
    pub drag: Option<Drag>,
    /// The mouse is dragging the brightness slider.
    pub slider_drag: bool,
    /// View used while dragging, so the map does not rescale under the mouse.
    pub frozen_view: Option<View>,
    pub view: Option<View>,
    pub confirm: Option<Confirm>,
    /// When the Identify badges should be removed.
    pub identify_until: Option<Instant>,
    pub status: Option<String>,
    /// The built-in panel's backlight, if this machine has one.
    pub backlight: Option<rsds_logind::Backlight>,
    pub hits: Hits,
    /// Identities of the monitors in use (see `active_outputs`), to notice
    /// monitors coming and going (including the panel when the lid closes).
    monitor_set: Vec<String>,
    /// Places for monitors that are off or duplicating to go back to, in
    /// the pending config.
    restores: Vec<SavedRestore>,
    /// The same for the kept config (as in the saved profile).
    kept_restores: Vec<SavedRestore>,
}

impl App {
    pub fn new(outputs: &[Output], backlight: Option<rsds_logind::Backlight>) -> Self {
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
            section: Section::Displays,
            nightlight: Nightlight::default(),
            lid_close: LidCloseSettings::default(),
            daemon_running: false,
            setting: Setting::Enabled,
            time_column: TimeColumn::Hour,
            typed_digits: 0,
            drag: None,
            slider_drag: false,
            frozen_view: None,
            view: None,
            confirm: None,
            identify_until: None,
            status: None,
            backlight,
            hits: Hits::default(),
            monitor_set: monitor_set(outputs),
            restores: Vec::new(),
            kept_restores: Vec::new(),
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
        let mut clean = Vec::new();
        for o in outputs {
            match self.entries.iter_mut().position(|e| e.live.name == o.name) {
                Some(i) => {
                    let e = &mut self.entries[i];
                    let was_clean = !e.is_dirty();
                    e.live = o.clone();
                    if was_clean {
                        e.pending = o.config();
                        clean.push(i);
                    }
                }
                None => {
                    clean.push(self.entries.len());
                    self.entries.push(Entry::new(next_number, o.clone()));
                    next_number += 1;
                }
            }
        }
        let live = self.live_duplicates(&self.current_sources());
        for i in clean {
            self.entries[i].duplicates = live[i].clone();
        }
        self.resolve_duplicates();
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
        if self.drag.as_ref().is_some_and(|d| d.index >= self.entries.len()) {
            self.drag = None;
            self.frozen_view = None;
        }
    }

    /// For each entry, the index of the entry it duplicates in the pending
    /// config.
    fn current_sources(&self) -> Vec<Option<usize>> {
        self.entries
            .iter()
            .map(|e| e.duplicates.as_ref().and_then(|s| self.entries.iter().position(|x| &x.live.name == s)))
            .collect()
    }

    /// Which entry each one duplicates in the live state, preferring the
    /// sources in `hint` where it's ambiguous.
    fn live_duplicates(&self, hint: &[Option<usize>]) -> Vec<Option<String>> {
        let live: Vec<Output> = self.entries.iter().map(|e| e.live.clone()).collect();
        duplicate_sources(&live, hint)
            .into_iter()
            .map(|s| s.map(|s| self.entries[s].live.name.clone()))
            .collect()
    }

    /// Take which monitor duplicates which from the saved profile for the
    /// current monitors, which tells them apart when they are the same size.
    pub fn use_profile(&mut self, profile: &Profile) {
        let live: Vec<Output> = self.entries.iter().map(|e| e.live.clone()).collect();
        let hint = profile.duplicate_sources(&live);
        let duplicates = self.live_duplicates(&hint);
        for (e, d) in self.entries.iter_mut().zip(duplicates) {
            if !e.is_dirty() {
                e.duplicates = d;
            }
        }
        self.kept_restores = profile.restores.clone();
        if !self.is_dirty() {
            self.restores = self.kept_restores.clone();
        }
    }

    /// For each entry, the index of the output it duplicates, for saving
    /// in a profile of `outputs`.
    pub fn duplicate_hint(&self, outputs: &[Output]) -> Vec<Option<usize>> {
        outputs
            .iter()
            .map(|o| {
                let e = self.entries.iter().find(|e| e.live.name == o.name)?;
                let source = e.duplicates.as_ref()?;
                outputs.iter().position(|x| &x.name == source)
            })
            .collect()
    }

    /// The entry the given entry duplicates, if any.
    pub fn source_of(&self, index: usize) -> Option<usize> {
        let source = self.entries.get(index)?.duplicates.as_ref()?;
        self.entries.iter().position(|e| &e.live.name == source)
    }

    /// Entries that duplicate the given one.
    pub fn duplicates_of(&self, index: usize) -> Vec<usize> {
        (0..self.entries.len()).filter(|&j| self.source_of(j) == Some(index)).collect()
    }

    /// Rectangle of an entry in the arrangement, which duplicates don't
    /// take part in: they sit on their sources.
    fn layout_rect(&self, index: usize) -> Option<Rect> {
        let e = &self.entries[index];
        if e.duplicates.is_some() { None } else { e.rect() }
    }

    /// Put each duplicate on its source and scale it to show the source's
    /// area. A duplicate whose source is gone, off, or a duplicate itself,
    /// or that is off itself, stops duplicating.
    fn resolve_duplicates(&mut self) {
        for i in 0..self.entries.len() {
            let source = self.source_of(i).filter(|&s| self.entries[s].duplicates.is_none());
            let source_rect = source.and_then(|s| self.entries[s].rect());
            let e = &mut self.entries[i];
            if e.duplicates.is_none() {
                continue;
            }
            match (source_rect, e.pending.mode.filter(|_| e.pending.enabled)) {
                // Already in place (e.g. as the compositor rounded it).
                (Some(r), Some(_)) if e.rect().is_some_and(|own| mirror::is_duplicate_of(own, r)) => {}
                (Some(r), Some(mode)) => {
                    e.pending.position = (r.x, r.y);
                    e.pending.scale = mirror::scale_to_cover(&mode, e.pending.transform, r);
                }
                _ => e.duplicates = None,
            }
        }
    }

    /// Stop entry `index` duplicating: it gets the nearest regular scale
    /// and goes to the right of the others.
    fn stop_duplicating(&mut self, index: usize) {
        let e = &mut self.entries[index];
        if e.duplicates.take().is_none() {
            return;
        }
        let scale = e.pending.scale;
        e.pending.scale = SCALES.into_iter().min_by(|a, b| (a - scale).abs().total_cmp(&(b - scale).abs())).unwrap_or(1.0);
        self.place_right(index);
    }

    /// Put entry `index` to the right of the others, centred on its
    /// neighbour there.
    fn place_right(&mut self, index: usize) {
        let h = self.entries[index].rect().map_or(0, |r| r.h);
        self.entries[index].pending.position = layout::place_right(&self.other_rects(index), h);
    }

    /// Identity of the monitor entry `index` duplicates, if any.
    fn source_identity(&self, index: usize) -> Option<String> {
        self.source_of(index).map(|s| self.entries[s].live.identity())
    }

    /// Every entry's pending config and source, leaving out `except`.
    fn snapshot(&self, except: Option<usize>) -> Vec<SavedOutput> {
        (0..self.entries.len())
            .filter(|&j| Some(j) != except)
            .map(|j| SavedOutput {
                identity: self.entries[j].live.identity(),
                config: self.entries[j].pending.clone(),
                duplicates: self.source_identity(j),
            })
            .collect()
    }

    /// Entry `index` just left the arrangement, which was `before`.
    fn record_leaving(&mut self, index: usize, before: Vec<SavedOutput>) {
        let identity = self.entries[index].live.identity();
        let after = self.snapshot(Some(index));
        self.restores.retain(|r| r.identity != identity);
        self.restores.push(SavedRestore { identity, before, after });
    }

    /// Whether the monitors other than the one `r` is for are where they
    /// were right after it left, the same size and duplicating the same.
    fn unchanged_since(&self, r: &SavedRestore) -> bool {
        let others = || self.entries.iter().enumerate().filter(|(_, e)| e.live.identity() != r.identity);
        r.after.iter().all(|s| self.entries.iter().any(|e| e.live.identity() == s.identity))
            && others().all(|(j, e)| match r.after.iter().find(|s| s.identity == e.live.identity()) {
                Some(s) => s.config.rect() == e.rect() && s.duplicates == self.source_identity(j),
                // Not in the saved arrangement (the panel, with the lid
                // closed), which is fine while it's off.
                None => !e.pending.enabled,
            })
    }

    /// Entry `index` is coming back to the arrangement: if nothing else
    /// changed since it left, put it and the others back as they were.
    /// Returns whether it did.
    fn restore_place(&mut self, index: usize) -> bool {
        let identity = self.entries[index].live.identity();
        let Some(k) = self.restores.iter().position(|r| r.identity == identity) else { return false };
        let r = self.restores.remove(k);
        if !self.unchanged_since(&r) {
            return false;
        }
        let names: Vec<(String, String)> =
            self.entries.iter().map(|e| (e.live.identity(), e.live.name.clone())).collect();
        let name_of = |id: &String| names.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone());
        for e in self.entries.iter_mut() {
            let Some(saved) = r.before.iter().find(|s| s.identity == e.live.identity()) else { continue };
            e.pending.position = saved.config.position;
            e.pending.scale = saved.config.scale;
            // Its other settings are as they are now.
            if saved.identity != identity {
                e.duplicates = saved.duplicates.as_ref().and_then(name_of);
            }
        }
        true
    }

    /// Places to go back to, for a profile of `outputs`.
    pub fn restores_for(&self, outputs: &[Output]) -> Vec<SavedRestore> {
        let ids: Vec<String> = outputs.iter().map(Output::identity).collect();
        let keep = |v: &[SavedOutput]| v.iter().filter(|s| ids.contains(&s.identity)).cloned().collect();
        self.kept_restores
            .iter()
            .filter(|r| ids.contains(&r.identity))
            .map(|r| SavedRestore { identity: r.identity.clone(), before: keep(&r.before), after: keep(&r.after) })
            .collect()
    }

    /// Identities of the monitors in use.
    pub fn monitor_set(&self) -> &[String] {
        &self.monitor_set
    }

    /// Label of a choice, naming the monitor a Duplicate choice refers to.
    pub fn choice_label(&self, choice: &Choice, e: &Entry, short: bool) -> String {
        match choice {
            Choice::Duplicate(Some(n)) => match self.entries.iter().find(|x| x.number == *n) {
                Some(x) => format!("{n} {}", x.live.name),
                None => n.to_string(),
            },
            c => c.label(e, short),
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

    /// Load the Nightlight and Lid Close settings and check for the daemon.
    pub fn load_settings(&mut self) {
        match Nightlight::load() {
            Ok(n) => self.nightlight = n,
            Err(e) => self.status = Some(format!("Couldn't read Nightlight settings: {e}")),
        }
        match LidCloseSettings::load() {
            Ok(s) => self.lid_close = s,
            Err(e) => self.status = Some(format!("Couldn't read Lid Close settings: {e}")),
        }
        self.daemon_running = rsds_core::daemon_lock::is_running();
    }

    fn save_lid_close(&mut self) {
        if let Err(e) = self.lid_close.save() {
            self.status = Some(format!("Couldn't save Lid Close settings: {e}"));
        }
    }

    fn save_nightlight(&mut self) {
        if let Err(e) = self.nightlight.save() {
            self.status = Some(format!("Couldn't save Nightlight settings: {e}"));
        }
    }

    /// Set the Nightlight colour temperature (clamped and rounded to a step).
    pub fn set_temperature(&mut self, kelvin: i64) {
        let step = TEMPERATURE_STEP as i64;
        let k = ((kelvin + step / 2) / step * step).clamp(nightlight::MIN_K as i64, nightlight::MAX_K as i64);
        if k as u32 != self.nightlight.temperature {
            self.nightlight.temperature = k as u32;
            self.save_nightlight();
        }
    }

    /// Fraction 0..=1 of the temperature slider.
    pub fn temperature_fraction(&self) -> f64 {
        let (lo, hi) = (nightlight::MIN_K as f64, nightlight::MAX_K as f64);
        (self.nightlight.temperature as f64 - lo) / (hi - lo)
    }

    /// Set the slider of the current setting from a fraction 0..=1.
    pub fn set_slider(&mut self, fraction: f64) {
        match self.current_setting() {
            Setting::Brightness => self.set_brightness((fraction * 100.0).round() as i32),
            Setting::Temperature => {
                let (lo, hi) = (nightlight::MIN_K as f64, nightlight::MAX_K as f64);
                self.set_temperature((lo + fraction * (hi - lo)).round() as i64)
            }
            _ => {}
        }
    }

    pub fn set_section(&mut self, section: Section) {
        if section != self.section {
            self.section = section;
            if section != Section::Displays {
                self.daemon_running = rsds_core::daemon_lock::is_running();
            }
        }
    }

    /// Move focus one pane right (`delta` 1) or left (-1), without wrapping.
    /// In the time picker, first moves between its hour and minute columns.
    pub fn move_focus(&mut self, delta: i32) {
        if self.time_picker_focused() {
            match (self.time_column, delta.signum()) {
                (TimeColumn::Hour, 1) => return self.time_column = TimeColumn::Minute,
                (TimeColumn::Minute, -1) => return self.time_column = TimeColumn::Hour,
                (TimeColumn::Minute, 1) => return,
                _ => {}
            }
        }
        let panes = self.section.panes();
        let i = panes.iter().position(|p| *p == self.focus).unwrap_or(0) as i32;
        self.set_focus(panes[(i + delta).clamp(0, panes.len() as i32 - 1) as usize]);
    }

    /// Cycle focus through the section's panes (Tab / Shift+Tab).
    pub fn cycle_focus(&mut self, delta: i32) {
        let panes = self.section.panes();
        let i = panes.iter().position(|p| *p == self.focus).unwrap_or(0) as i32;
        self.set_focus(panes[(i + delta).rem_euclid(panes.len() as i32) as usize]);
    }

    /// Focus `pane`; entering the options pane starts at the time picker's
    /// hour column.
    fn set_focus(&mut self, pane: Focus) {
        if pane == Focus::Options && self.focus != Focus::Options {
            self.time_column = TimeColumn::Hour;
        }
        self.focus = pane;
    }

    /// Whether the options pane shows the schedule's time picker.
    pub fn shows_time_picker(&self) -> bool {
        self.section == Section::Nightlight
            && matches!(self.current_setting(), Setting::ScheduleStart | Setting::ScheduleEnd)
    }

    fn time_picker_focused(&self) -> bool {
        self.focus == Focus::Options && self.shows_time_picker()
    }

    /// The schedule time the time picker shows.
    pub fn picker_time(&self) -> TimeOfDay {
        match self.current_setting() {
            Setting::ScheduleEnd => self.nightlight.end,
            _ => self.nightlight.start,
        }
    }

    /// Set one column of the time picker's time, clamped to its largest
    /// value, and save it.
    pub fn set_time_part(&mut self, column: TimeColumn, value: u16) {
        let t = self.picker_time();
        let (h, m) = match column {
            TimeColumn::Hour => (value.min(23), t.minute()),
            TimeColumn::Minute => (t.hour(), value.min(59)),
        };
        let t = TimeOfDay(h * 60 + m);
        match self.current_setting() {
            Setting::ScheduleStart => self.nightlight.start = t,
            Setting::ScheduleEnd => self.nightlight.end = t,
            _ => return,
        }
        self.save_nightlight();
    }

    /// A digit typed in the time picker. Two digits fill a column (too
    /// large a value becomes the largest valid one); after the hour, typing
    /// continues in the minute column.
    pub fn type_digit(&mut self, digit: u16) {
        if !self.time_picker_focused() {
            return;
        }
        let column = self.time_column;
        let value = if self.typed_digits == 0 {
            digit
        } else {
            let t = self.picker_time();
            let first = if column == TimeColumn::Hour { t.hour() } else { t.minute() };
            first * 10 + digit
        };
        self.set_time_part(column, value.min(column.max()));
        self.typed_digits += 1;
        if self.typed_digits == 2 {
            self.typed_digits = 0;
            self.time_column = TimeColumn::Minute;
        }
    }

    /// Anything other than a digit ends typing into the time picker.
    pub fn end_typing(&mut self) {
        self.typed_digits = 0;
    }

    /// Settings shown in the settings pane: for the selected monitor, or
    /// Nightlight's.
    pub fn settings(&self) -> Vec<Setting> {
        match self.section {
            Section::Nightlight => return NIGHTLIGHT_SETTINGS.to_vec(),
            Section::LidClose => return LID_CLOSE_SETTINGS.to_vec(),
            Section::Displays => {}
        }
        let mut v = DISPLAY_SETTINGS.to_vec();
        if self.backlight.is_some() && self.selected_entry().is_some_and(|e| e.live.is_internal()) {
            v.insert(1, Setting::Brightness);
        }
        v
    }

    /// The selected setting, falling back to the first one if the selected
    /// monitor doesn't have it (e.g. Brightness on an external monitor).
    pub fn current_setting(&self) -> Setting {
        let settings = self.settings();
        if settings.contains(&self.setting) { self.setting } else { settings[0] }
    }

    pub fn move_setting(&mut self, delta: i32) {
        let settings = self.settings();
        let idx = settings.iter().position(|s| *s == self.current_setting()).unwrap_or(0);
        self.setting = settings[(idx as i32 + delta).rem_euclid(settings.len() as i32) as usize];
    }

    /// Brightness of the built-in panel in percent, read fresh from sysfs.
    pub fn brightness(&self) -> Option<u8> {
        self.backlight.as_ref()?.percent().ok()
    }

    /// Set the built-in panel's brightness immediately.
    pub fn set_brightness(&mut self, percent: i32) {
        let Some(b) = self.backlight.as_mut() else { return };
        if let Err(e) = b.set_percent(percent.clamp(0, 100) as u8) {
            self.status = Some(format!("Couldn't set brightness: {e}"));
        }
    }

    /// The available values of the current setting for the selected
    /// monitor, in display order, and the index of the pending value.
    pub fn choices(&self) -> (Vec<Choice>, Option<usize>) {
        match self.section {
            Section::Nightlight => return self.nightlight_choices(),
            Section::LidClose => {
                let current = LidClose::ALL.iter().position(|l| *l == self.lid_close.lid_close);
                return (LidClose::ALL.into_iter().map(Choice::LidClose).collect(), current);
            }
            Section::Displays => {}
        }
        let Some(e) = self.selected_entry() else { return (Vec::new(), None) };
        let setting = self.current_setting();
        if !e.pending.enabled && setting != Setting::Enabled {
            return (Vec::new(), None);
        }
        if setting == Setting::Duplicate {
            let mut choices = vec![Choice::Duplicate(None)];
            choices.extend(
                self.entries
                    .iter()
                    .enumerate()
                    .filter(|(j, x)| *j != self.selected && x.pending.enabled && x.duplicates.is_none())
                    .map(|(_, x)| Choice::Duplicate(Some(x.number))),
            );
            let source = self.source_of(self.selected).map(|s| self.entries[s].number);
            let current = choices.iter().position(|c| *c == Choice::Duplicate(source));
            return (choices, current);
        }
        let choices: Vec<Choice> = match setting {
            Setting::Enabled => vec![Choice::Enabled(true), Choice::Enabled(false)],
            // A duplicate's scale follows from its source.
            Setting::Scale if e.duplicates.is_some() => Vec::new(),
            // Shown as a slider rather than a list.
            Setting::Brightness => Vec::new(),
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
            _ => Vec::new(),
        };
        let current = choices.iter().position(|c| c.is_current(e));
        (choices, current)
    }

    fn nightlight_choices(&self) -> (Vec<Choice>, Option<usize>) {
        let n = &self.nightlight;
        let on_off = |v: bool| (vec![Choice::Enabled(true), Choice::Enabled(false)], Some(if v { 0 } else { 1 }));
        match self.current_setting() {
            Setting::NightlightEnabled => on_off(n.enabled),
            Setting::Schedule => on_off(n.schedule),
            // Temperature is a slider; the times have their own picker.
            _ => (Vec::new(), None),
        }
    }

    /// Up/down within the focused pane.
    pub fn move_in_focus(&mut self, delta: i32) {
        match self.focus {
            Focus::Sections => {
                let i = SECTIONS.iter().position(|s| *s == self.section).unwrap_or(0) as i32;
                self.set_section(SECTIONS[(i + delta).clamp(0, SECTIONS.len() as i32 - 1) as usize]);
            }
            Focus::Displays => self.move_selection(delta),
            Focus::Settings => self.move_setting(delta),
            Focus::Options => self.change_setting(delta),
        }
    }

    /// Step the current setting of the selected monitor through its choices.
    /// For brightness, up (negative delta) is brighter.
    pub fn change_setting(&mut self, delta: i32) {
        if self.current_setting() == Setting::Brightness {
            if let Some(p) = self.brightness() {
                // Snap to the step grid so repeated presses land on round values.
                let p = p as i32;
                let next = if delta < 0 {
                    (p / BRIGHTNESS_STEP + 1) * BRIGHTNESS_STEP
                } else {
                    (p + BRIGHTNESS_STEP - 1) / BRIGHTNESS_STEP * BRIGHTNESS_STEP - BRIGHTNESS_STEP
                };
                self.set_brightness(next);
            }
            return;
        }
        if self.shows_time_picker() {
            let t = self.picker_time();
            let (column, value) = match self.time_column {
                TimeColumn::Hour => (TimeColumn::Hour, step_hour(t.hour(), delta)),
                TimeColumn::Minute => (TimeColumn::Minute, step_minute(t.minute(), delta)),
            };
            self.set_time_part(column, value);
            return;
        }
        if self.current_setting() == Setting::Temperature {
            // Up (negative delta) is a higher temperature: cooler, less tint.
            let k = self.nightlight.temperature as i64 - delta as i64 * TEMPERATURE_STEP as i64;
            self.set_temperature(k);
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
        if self.section == Section::Nightlight {
            let setting = self.current_setting();
            let n = &mut self.nightlight;
            match (setting, choice) {
                (Setting::NightlightEnabled, Choice::Enabled(on)) => n.enabled = on,
                (Setting::Schedule, Choice::Enabled(on)) => n.schedule = on,
                _ => return,
            }
            self.save_nightlight();
            return;
        }
        if let Choice::LidClose(l) = choice {
            self.lid_close.lid_close = l;
            self.save_lid_close();
            return;
        }
        let i = self.selected;
        if let Choice::Duplicate(target) = choice {
            self.set_duplicate(i, target);
            return;
        }
        let before = self.entries[i].rect();
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
            // Handled above.
            Choice::Duplicate(_) | Choice::LidClose(_) => return,
            Choice::Refresh(mode) => {
                e.pending.mode = Some(mode);
                return; // size unchanged
            }
            Choice::Orientation(t) => e.pending.transform = t,
            Choice::Scale(scale) => e.pending.scale = scale,
        }
        self.fixup_layout(i, before);
    }

    /// Make entry `index` duplicate the monitor numbered `target`, or stop
    /// duplicating. Monitors duplicating it then duplicate `target` too.
    fn set_duplicate(&mut self, index: usize, target: Option<usize>) {
        let Some(target) = target else {
            if self.entries[index].duplicates.is_some() && self.restore_place(index) {
                self.entries[index].duplicates = None;
            } else {
                self.stop_duplicating(index);
            }
            self.fixup_layout(index, None);
            return;
        };
        let Some(source) = self.entries.iter().position(|e| e.number == target) else { return };
        if source == index || self.entries[source].duplicates.is_some() {
            return;
        }
        let leaving = self.entries[index].duplicates.is_none();
        let before = self.snapshot(None);
        let name = self.entries[source].live.name.clone();
        for j in self.duplicates_of(index) {
            self.entries[j].duplicates = Some(name.clone());
        }
        self.entries[index].duplicates = Some(name);
        // It leaves its place in the arrangement: close the gap.
        self.fixup_layout(source, None);
        if leaving {
            self.record_leaving(index, before);
        }
    }

    pub fn toggle_enabled(&mut self) {
        let i = self.selected;
        let enabled_count = self.entries.iter().filter(|e| e.pending.enabled).count();
        let before = self.snapshot(None);
        let Some(e) = self.entries.get_mut(i) else { return };
        if e.pending.enabled {
            if enabled_count <= 1 {
                self.status = Some("Can't turn off the only active display".into());
                return;
            }
            // A duplicate already left the arrangement when it became one.
            let leaving = e.duplicates.is_none();
            e.pending.enabled = false;
            e.duplicates = None;
            // Its duplicates become monitors of their own.
            for j in self.duplicates_of(i) {
                self.stop_duplicating(j);
            }
            self.fixup_layout(i, None);
            if leaving {
                self.record_leaving(i, before);
            }
            return;
        } else {
            e.pending.enabled = true;
            if e.pending.mode.is_none() {
                e.pending.mode = e.live.preferred_mode().copied();
            }
            if e.pending.scale <= 0.0 {
                e.pending.scale = 1.0;
            }
            // Turned-on monitors go back to where they were, or else to
            // the right of the others.
            if !self.restore_place(i) {
                self.place_right(i);
            }
        }
        self.fixup_layout(i, None);
    }

    /// Enabled monitor rects other than `index`, leaving out duplicates.
    fn other_rects(&self, index: usize) -> Vec<Rect> {
        (0..self.entries.len()).filter(|&j| j != index).filter_map(|j| self.layout_rect(j)).collect()
    }

    /// After monitor `changed` changed size or was turned on/off, move the
    /// others so nothing overlaps and nothing floats detached. `before` is
    /// its rect before a resize, so its neighbours can follow its edges.
    fn fixup_layout(&mut self, changed: usize, before: Option<Rect>) {
        if self.entries[changed].duplicates.is_some() {
            // It sits on its source; only its scale changes.
            self.resolve_duplicates();
            return;
        }
        let anchor = self.entries[changed].rect();
        let centre = anchor
            .map(|r| (r.x + r.w / 2, r.y + r.h / 2))
            .unwrap_or(self.entries[changed].pending.position);
        let (indices, mut rects): (Vec<usize>, Vec<Rect>) = (0..self.entries.len())
            .filter(|&j| j != changed)
            .filter_map(|j| self.layout_rect(j).map(|r| (j, r)))
            .unzip();
        if let (Some(old), Some(new)) = (before, anchor) {
            layout::follow_resize(old, new, &mut rects);
        }
        let all: Vec<Rect> = anchor.into_iter().chain(rects.iter().copied()).collect();
        if !layout::is_valid(&all) {
            rects = layout::compact(anchor, centre, &rects);
        }
        for (j, r) in indices.into_iter().zip(rects) {
            self.entries[j].pending.position = (r.x, r.y);
        }
        self.normalize();
        self.resolve_duplicates();
    }

    /// Shift all enabled monitors so the layout starts at (0, 0).
    fn normalize(&mut self) {
        let rects: Vec<Rect> = (0..self.entries.len()).filter_map(|j| self.layout_rect(j)).collect();
        let (dx, dy) = layout::normalize_offset(&rects);
        for e in self.entries.iter_mut().filter(|e| e.pending.enabled) {
            e.pending.position.0 += dx;
            e.pending.position.1 += dy;
        }
    }

    pub fn begin_drag(&mut self, index: usize, cell: (u16, u16)) {
        let index = self.source_of(index).unwrap_or(index);
        self.select(index);
        self.set_section(Section::Displays);
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

    /// The monitor that entry `index`, at `r`, is dropped right on top of:
    /// its centre is in the middle half (across and down) of that monitor.
    fn drop_target(&self, index: usize, r: Rect) -> Option<usize> {
        let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
        (0..self.entries.len()).filter(|&j| j != index).find(|&j| {
            self.layout_rect(j).is_some_and(|t| {
                (cx - (t.x + t.w / 2)).abs() <= t.w / 4 && (cy - (t.y + t.h / 2)).abs() <= t.h / 4
            })
        })
    }

    pub fn end_drag(&mut self) {
        let Some(d) = self.drag.take() else { return };
        let view = self.frozen_view.take();
        if !d.moved {
            return;
        }
        let Some(rect) = self.entries[d.index].rect() else { return };
        if let Some(target) = self.drop_target(d.index, rect) {
            // Back where it was first, so that it returns there if it
            // stops duplicating.
            self.entries[d.index].pending.position = d.start_pos;
            let number = self.entries[target].number;
            self.set_duplicate(d.index, Some(number));
            return;
        }
        let threshold = view.map_or((0, 0), View::snap_threshold);
        let snapped = layout::snap(rect, &self.other_rects(d.index), threshold);
        self.entries[d.index].pending.position = (snapped.x, snapped.y);
        self.fixup_layout(d.index, None);
    }

    /// Discard unapplied edits.
    pub fn reset(&mut self) {
        self.adopt_live();
        self.restores = self.kept_restores.clone();
    }

    /// Make the pending config the live one, e.g. after applying it.
    pub fn adopt_live(&mut self) {
        self.reset_pending();
        self.status = None;
    }

    fn reset_pending(&mut self) {
        let live = self.live_duplicates(&self.current_sources());
        for (e, d) in self.entries.iter_mut().zip(live) {
            e.pending = e.live.config();
            e.duplicates = d;
        }
    }

    pub fn apply(&mut self) -> Option<Request> {
        if self.confirm.is_some() || !self.is_dirty() {
            return None;
        }
        // Places that are already gone stay gone, whatever the compositor
        // does to the layout.
        let restores = std::mem::take(&mut self.restores);
        self.restores = restores.into_iter().filter(|r| self.unchanged_since(r)).collect();
        Some(Request::Apply(self.entries.iter().map(|e| e.pending.clone()).collect()))
    }

    /// Called after a user-initiated apply succeeded.
    pub fn applied(&mut self, previous: Vec<OutputConfig>) {
        self.status = None;
        // For the daemon, in case monitors change during the countdown.
        if let Err(e) = unconfirmed::save(&previous, REVERT_AFTER) {
            self.status = Some(format!("Couldn't record the previous configuration: {e}"));
        }
        self.confirm = Some(Confirm { deadline: Instant::now() + REVERT_AFTER, previous });
    }

    /// The user confirmed the new configuration: remember it.
    pub fn keep(&mut self) -> Request {
        self.confirm = None;
        unconfirmed::clear();
        // The compositor may have rounded the layout a little.
        for k in 0..self.restores.len() {
            let identity = &self.restores[k].identity;
            let index = self.entries.iter().position(|e| &e.live.identity() == identity);
            self.restores[k].after = self.snapshot(index);
        }
        self.kept_restores = self.restores.clone();
        Request::SaveProfile
    }

    pub fn revert(&mut self) -> Option<Request> {
        let c = self.confirm.take()?;
        unconfirmed::clear();
        self.restores = self.kept_restores.clone();
        for e in &mut self.entries {
            if let Some(p) = c.previous.iter().find(|p| p.name == e.pending.name) {
                e.pending = p.clone();
            }
        }
        let rects: Vec<Option<Rect>> = self.entries.iter().map(Entry::rect).collect();
        let sources = mirror::infer(&rects, &self.current_sources());
        for i in 0..self.entries.len() {
            self.entries[i].duplicates = sources[i].map(|s| self.entries[s].live.name.clone());
        }
        Some(Request::Revert(c.previous))
    }

    /// The display server reported a change we didn't make. `lid_closed`:
    /// the lid is closed and that takes the panel out of use.
    ///
    /// If monitors came or went, unapplied edits are dropped and any
    /// confirmation countdown ends: the daemon reverts the unconfirmed
    /// configuration while handling the change. Without the daemon, the
    /// returned request reverts it.
    pub fn outputs_changed(&mut self, outputs: &[Output], lid_closed: bool) -> Option<Request> {
        let set = monitor_set(&active_outputs(outputs, lid_closed));
        self.sync(outputs);
        if set == self.monitor_set {
            return None;
        }
        self.monitor_set = set;
        let confirm = self.confirm.take();
        if confirm.is_some() || self.is_dirty() {
            self.status = Some("Monitors changed: unapplied changes discarded".into());
        }
        self.drag = None;
        self.frozen_view = None;
        self.reset_pending();
        // The new set's places come with its profile (see `use_profile`).
        self.restores.clear();
        self.kept_restores.clear();
        let previous = confirm?.previous;
        if rsds_core::daemon_lock::is_running() {
            return None;
        }
        unconfirmed::clear();
        let present: Vec<OutputConfig> =
            previous.into_iter().filter(|c| outputs.iter().any(|o| o.name == c.name)).collect();
        Some(Request::Revert(present))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_wrap() {
        assert_eq!(step_hour(23, 1), 0);
        assert_eq!(step_hour(0, -2), 22);
        assert_eq!(step_hour(12, 0), 12);
    }

    #[test]
    fn minutes_step_on_grid_and_wrap() {
        assert_eq!(step_minute(55, 1), 0);
        assert_eq!(step_minute(0, -1), 55);
        assert_eq!(step_minute(7, 1), 10);
        assert_eq!(step_minute(7, -1), 5);
        assert_eq!(step_minute(7, -2), 0);
        assert_eq!(step_minute(7, 0), 7);
    }
}
