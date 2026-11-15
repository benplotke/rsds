//! UI state and the operations the user can perform on it.

use std::time::{Duration, Instant};

use rsds_core::{
    Mode, Output, OutputConfig, Transform,
    nightlight::{self, Nightlight, TimeOfDay},
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
    /// The left-most pane: Displays / Nightlight.
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
}

pub const SECTIONS: [Section; 2] = [Section::Displays, Section::Nightlight];

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Displays => "Displays",
            Section::Nightlight => "Nightlight",
        }
    }

    /// The panes shown for this section, left to right.
    pub fn panes(self) -> &'static [Focus] {
        match self {
            Section::Displays => &[Focus::Sections, Focus::Displays, Focus::Settings, Focus::Options],
            Section::Nightlight => &[Focus::Sections, Focus::Settings, Focus::Options],
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
    // Nightlight section; applied immediately (saved for the daemon).
    NightlightEnabled,
    Temperature,
    Schedule,
    ScheduleStart,
    ScheduleEnd,
}

const NIGHTLIGHT_SETTINGS: [Setting; 5] = [
    Setting::NightlightEnabled,
    Setting::Temperature,
    Setting::Schedule,
    Setting::ScheduleStart,
    Setting::ScheduleEnd,
];

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

const DISPLAY_SETTINGS: [Setting; 5] = [
    Setting::Enabled,
    Setting::Resolution,
    Setting::Refresh,
    Setting::Orientation,
    Setting::Scale,
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
}

pub struct App {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub focus: Focus,
    pub section: Section,
    /// Nightlight settings, saved for the daemon on every change.
    pub nightlight: Nightlight,
    /// Whether the daemon (which applies Nightlight) is running.
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

    /// Load the Nightlight settings and check for the daemon.
    pub fn load_nightlight(&mut self) {
        match Nightlight::load() {
            Ok(n) => self.nightlight = n,
            Err(e) => self.status = Some(format!("Couldn't read Nightlight settings: {e}")),
        }
        self.daemon_running = rsds_core::daemon_lock::is_running();
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
            if section == Section::Nightlight {
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
        if self.section == Section::Nightlight {
            return NIGHTLIGHT_SETTINGS.to_vec();
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
        if self.section == Section::Nightlight {
            return self.nightlight_choices();
        }
        let Some(e) = self.selected_entry() else { return (Vec::new(), None) };
        let setting = self.current_setting();
        if !e.pending.enabled && setting != Setting::Enabled {
            return (Vec::new(), None);
        }
        let choices: Vec<Choice> = match setting {
            Setting::Enabled => vec![Choice::Enabled(true), Choice::Enabled(false)],
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
        let i = self.selected;
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
            Choice::Refresh(mode) => {
                e.pending.mode = Some(mode);
                return; // size unchanged
            }
            Choice::Orientation(t) => e.pending.transform = t,
            Choice::Scale(scale) => e.pending.scale = scale,
        }
        self.fixup_layout(i, before);
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
        self.fixup_layout(i, None);
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
    /// others so nothing overlaps and nothing floats detached. `before` is
    /// its rect before a resize, so its neighbours can follow its edges.
    fn fixup_layout(&mut self, changed: usize, before: Option<Rect>) {
        let anchor = self.entries[changed].rect();
        let centre = anchor
            .map(|r| (r.x + r.w / 2, r.y + r.h / 2))
            .unwrap_or(self.entries[changed].pending.position);
        let (indices, mut rects): (Vec<usize>, Vec<Rect>) = self
            .entries
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != changed)
            .filter_map(|(j, e)| e.rect().map(|r| (j, r)))
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
        self.fixup_layout(d.index, None);
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
