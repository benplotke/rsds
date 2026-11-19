//! Rendering. Also records clickable regions in `App::hits`.

use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
};
use rsds_core::{layout::Rect as LRect, lid_close::LidClose};

use crate::app::{
    self, App, Choice, Entry, Focus, Hits, SECTIONS, Section, Setting, TimeColumn, View,
};

// Colours loosely after btop's default theme.
const FG: Color = Color::Rgb(0xcc, 0xcc, 0xcc);
const TITLE: Color = Color::Rgb(0xee, 0xee, 0xee);
const INACTIVE: Color = Color::Rgb(0x60, 0x60, 0x60);
const HI: Color = Color::Rgb(0xd5, 0x50, 0x50);
const SELECTED_BG: Color = Color::Rgb(0x6a, 0x2f, 0x2f);
const LAYOUT_BOX: Color = Color::Rgb(0x55, 0x6d, 0x59);
const LIST_BOX: Color = Color::Rgb(0x5c, 0x58, 0x8d);
const SETTINGS_BOX: Color = Color::Rgb(0x80, 0x52, 0x52);
const OPTIONS_BOX: Color = Color::Rgb(0x6c, 0x6c, 0x4b);
const SECTIONS_BOX: Color = Color::Rgb(0x4b, 0x6c, 0x6c);
const INFO_BOX: Color = Color::Rgb(0x5c, 0x6c, 0x8d);
const MONITOR: Color = Color::Rgb(0x90, 0x90, 0x90);
const WARN: Color = Color::Rgb(0xe0, 0xb0, 0x40);
/// The letter of a button that is also its key.
const KEY: Color = Color::Rgb(0xff, 0x7a, 0x7a);

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hits = Hits::default();
    // Keep the bottom height stable when switching sections.
    let display_settings = app::DISPLAY_SETTINGS.len() + app.backlight.is_some() as usize;
    // Odd, so the time picker's current row can sit exactly in the middle.
    let rows = app
        .entries
        .len()
        .max(display_settings)
        .max(app.settings().len())
        | 1;
    let [top, bottom, footer] = Layout::vertical([
        Constraint::Min(8),
        Constraint::Length(rows as u16 + 2),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_arrangement(f, app, top);
    match app.section {
        Section::Displays => {
            let [sections, list, settings, options] = Layout::horizontal([
                Constraint::Length(SECTIONS_W),
                Constraint::Length(40),
                Constraint::Length(SETTINGS_W),
                Constraint::Min(20),
            ])
            .areas(bottom);
            draw_sections(f, app, sections);
            draw_list(f, app, list);
            draw_settings(f, app, settings);
            draw_options(f, app, options);
            app.hits.panes = vec![
                (Focus::Sections, sections),
                (Focus::Displays, list),
                (Focus::Settings, settings),
                (Focus::Options, options),
            ];
        }
        Section::LidClose => {
            let [sections, options, info] = Layout::horizontal([
                Constraint::Length(SECTIONS_W),
                Constraint::Length(LID_CLOSE_W),
                Constraint::Min(20),
            ])
            .areas(bottom);
            draw_sections(f, app, sections);
            draw_options(f, app, options);
            draw_lid_close_info(f, app, info);
            app.hits.panes = vec![(Focus::Sections, sections), (Focus::Options, options)];
        }
        Section::Nightlight => {
            let [sections, settings, options] = Layout::horizontal([
                Constraint::Length(SECTIONS_W),
                Constraint::Length(SETTINGS_W),
                Constraint::Min(20),
            ])
            .areas(bottom);
            draw_sections(f, app, sections);
            draw_settings(f, app, settings);
            draw_options(f, app, options);
            app.hits.panes = vec![
                (Focus::Sections, sections),
                (Focus::Settings, settings),
                (Focus::Options, options),
            ];
        }
    }
    draw_footer(f, app, footer);
    if app.confirm.is_some() {
        draw_confirm(f, app);
    }
}

/// Width of the sections pane, borders included.
const SECTIONS_W: u16 = 15;

fn draw_sections(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Sections;
    let block = boxed("rsds", SECTIONS_BOX, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    for (i, section) in SECTIONS.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let row = Rect {
            y: inner.y + i as u16,
            height: 1,
            ..inner
        };
        let mut style = Style::new().fg(FG);
        if *section == app.section {
            style = style
                .bg(if focused { SELECTED_BG } else { UNFOCUSED_BG })
                .add_modifier(Modifier::BOLD);
        }
        f.render_widget(
            Paragraph::new(pad_to(&format!(" {}", section.label()), row.width)).style(style),
            row,
        );
        app.hits.section_rows.push((*section, row));
    }
}

/// Width of the Lid Close choices pane.
const LID_CLOSE_W: u16 = 28;

/// What the chosen Lid Close option does.
fn lid_close_info(l: LidClose) -> &'static str {
    match l {
        LidClose::Suspend => {
            "Closing the lid suspends the computer, even with external monitors connected."
        }
        LidClose::SuspendUnlessDocked => {
            "Closing the lid suspends the computer, unless an external monitor is connected, \
             in which case only the built-in display turns off."
        }
        LidClose::Never => {
            "Closing the lid never suspends the computer. The built-in display just goes dark."
        }
        LidClose::SystemDefault => {
            "Whether closing the lid suspends is left to the system, e.g. HandleLidSwitch in \
             /etc/systemd/logind.conf."
        }
    }
}

fn draw_lid_close_info(f: &mut Frame, app: &App, area: Rect) {
    let block = boxed("info", INFO_BOX, false).padding(Padding::horizontal(1));
    let text = Paragraph::new(lid_close_info(app.lid_close.lid_close))
        .style(Style::new().fg(FG))
        .wrap(Wrap { trim: true })
        .block(block);
    f.render_widget(text, area);
}

fn boxed(title: &str, color: Color, focused: bool) -> Block<'_> {
    let title_style = if focused {
        Style::new().fg(TITLE).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TITLE)
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Line::from(vec![
            Span::styled("┤", Style::new().fg(color)),
            Span::styled(title, title_style),
            Span::styled("├", Style::new().fg(color)),
        ]))
}

fn draw_arrangement(f: &mut Frame, app: &mut App, area: Rect) {
    let block = boxed("arrangement", LAYOUT_BOX, false);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rects: Vec<Option<LRect>> = app.entries.iter().map(Entry::rect).collect();
    let shown: Vec<LRect> = rects.iter().flatten().copied().collect();
    // After a drag, the map keeps its scale while nothing else changes.
    let kept = app
        .kept_view
        .as_ref()
        .filter(|(v, kept)| v.area == inner && *kept == rects)
        .and_then(|(v, _)| v.showing(&shown));
    let view = match (app.frozen_view, kept) {
        (Some(v), _) | (None, Some(v)) => v,
        (None, None) => {
            app.kept_view = None;
            View::fit(inner, &shown)
        }
    };
    app.view = Some(view);

    // Monitor borders go on one grid so neighbours share a single line,
    // drawn heavy where they join. A monitor being dragged may overlap the
    // others, so it is drawn on top as its own box instead.
    let dragged = app.drag.as_ref().map(|d| d.index);
    let mut grid = BorderGrid::default();
    let mut boxes = Vec::new();
    // A duplicate is drawn as part of its source's box, which is selected
    // when either is.
    let selected = app.source_of(app.selected).unwrap_or(app.selected);
    // The selected monitor goes last so it wins clicks on shared edges.
    let mut order: Vec<usize> = (0..app.entries.len()).filter(|&i| i != selected).collect();
    order.push(selected);
    for i in order {
        let e = &app.entries[i];
        if e.duplicates.is_some() {
            continue;
        }
        let Some(rect) = e.rect() else { continue };
        let cells = view.to_cells(rect);
        if cells.width < 3 || cells.height < 3 {
            continue;
        }
        if Some(i) != dragged {
            let selected = i == selected;
            let (color, rank) = if selected { (HI, 1) } else { (MONITOR, 0) };
            grid.add(cells, if selected { 2 } else { 1 }, color, rank);
        }
        boxes.push((i, cells));
    }
    grid.render(f.buffer_mut());

    for (i, cells) in boxes {
        let e = &app.entries[i];
        let body = if Some(i) == dragged {
            let block = Block::bordered()
                .border_type(if i == selected {
                    BorderType::Thick
                } else {
                    BorderType::Rounded
                })
                .border_style(Style::new().fg(WARN));
            let body = block.inner(cells);
            f.render_widget(Clear, cells);
            f.render_widget(block, cells);
            body
        } else {
            Rect::new(cells.x + 1, cells.y + 1, cells.width - 2, cells.height - 2)
        };

        // A source and its duplicates, e.g. "1 | 3" and "eDP-1 | DP-6".
        let shown: Vec<&Entry> = std::iter::once(e)
            .chain(app.duplicates_of(i).into_iter().map(|j| &app.entries[j]))
            .collect();
        let join =
            |f: fn(&Entry) -> String| shown.iter().map(|x| f(x)).collect::<Vec<_>>().join(" | ");
        let mut lines = vec![
            Line::styled(
                join(|x| x.number.to_string()),
                Style::new().fg(TITLE).add_modifier(Modifier::BOLD),
            ),
            Line::styled(join(|x| x.live.name.clone()), Style::new().fg(FG)),
        ];
        if let Some(m) = e.pending.mode {
            lines.push(Line::styled(
                format!("{}×{}", m.width, m.height),
                Style::new().fg(INACTIVE),
            ));
        }
        lines.truncate(body.height as usize);
        let pad = body.height.saturating_sub(lines.len() as u16) / 2;
        let text_area = Rect {
            y: body.y + pad,
            height: body.height - pad,
            ..body
        };
        f.render_widget(
            Paragraph::new(lines).alignment(Alignment::Center),
            text_area,
        );
        app.hits.monitors.push((i, cells));
    }
}

/// Box-drawing glyphs indexed by arm weights (0 none, 1 light, 2 heavy) as
/// `up * 27 + down * 9 + left * 3 + right`.
const GLYPHS: &str =
    " ╶╺╴─╼╸╾━╷┌┍┐┬┮┑┭┯╻┎┏┒┰┲┓┱┳╵└┕┘┴┶┙┵┷│├┝┤┼┾┥┽┿╽┟┢┧╁╆┪╅╈╹┖┗┚┸┺┛┹┻╿┞┡┦╀╄┩╃╇┃┠┣┨╂╊┫╉╋";

/// One cell of a [`BorderGrid`].
#[derive(Default)]
struct Junction {
    /// Weights of the up, down, left and right arms.
    arms: [u8; 4],
    color: Option<(u8, Color)>,
}

/// Borders of several boxes drawn as one set of lines, so boxes sharing an
/// edge share a line, which is heavy where they meet.
#[derive(Default)]
struct BorderGrid {
    cells: std::collections::HashMap<(u16, u16), Junction>,
}

impl BorderGrid {
    /// Add the border of `r` with line `weight`. Where borders cross, the
    /// colour with the highest `rank` wins.
    fn add(&mut self, r: Rect, weight: u8, color: Color, rank: u8) {
        let (x0, y0, x1, y1) = (r.x, r.y, r.right() - 1, r.bottom() - 1);
        let mut arm = |x: u16, y: u16, a: usize| {
            let j = self.cells.entry((x, y)).or_default();
            j.arms[a] = if j.arms[a] > 0 { 2 } else { weight };
            if j.color.is_none_or(|(k, _)| rank >= k) {
                j.color = Some((rank, color));
            }
        };
        for x in x0..=x1 {
            for y in [y0, y1] {
                if x > x0 {
                    arm(x, y, 2);
                }
                if x < x1 {
                    arm(x, y, 3);
                }
            }
        }
        for y in y0..=y1 {
            for x in [x0, x1] {
                if y > y0 {
                    arm(x, y, 0);
                }
                if y < y1 {
                    arm(x, y, 1);
                }
            }
        }
    }

    fn render(&self, buf: &mut ratatui::buffer::Buffer) {
        for (&(x, y), j) in &self.cells {
            let [u, d, l, r] = j.arms;
            let glyph = match j.arms {
                // Plain corners keep the rounded look of the other boxes.
                [0, 1, 0, 1] => '╭',
                [0, 1, 1, 0] => '╮',
                [1, 0, 0, 1] => '╰',
                [1, 0, 1, 0] => '╯',
                _ => GLYPHS
                    .chars()
                    .nth((u * 27 + d * 9 + l * 3 + r) as usize)
                    .unwrap_or(' '),
            };
            let color = j.color.map_or(MONITOR, |(_, c)| c);
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(glyph).set_fg(color);
            }
        }
    }
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Displays;
    let block = boxed("displays", LIST_BOX, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    for (i, e) in app.entries.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let row = Rect {
            y: inner.y + i as u16,
            height: 1,
            ..inner
        };
        let name = format!("{} {}", e.live.make, e.live.model);
        let dirty = if e.is_dirty() { "*" } else { " " };
        let source = app.source_of(i).map(|s| app.entries[s].number);
        let state = match (e.pending.enabled, source) {
            (false, _) => " off".into(),
            (true, Some(n)) => format!(" dup of {n}"),
            (true, None) => String::new(),
        };
        let text = format!("{dirty}{:>2} {:<7} {name}{state}", e.number, e.live.name);
        let mut style = Style::new().fg(if e.pending.enabled { FG } else { INACTIVE });
        if i == app.selected {
            style = style.bg(if focused { SELECTED_BG } else { UNFOCUSED_BG });
        }
        f.render_widget(Paragraph::new(pad_to(&text, row.width)).style(style), row);
        app.hits.list_rows.push((i, row));
    }
}

/// Background for the selected row of a pane that doesn't have focus.
const UNFOCUSED_BG: Color = Color::Rgb(0x30, 0x30, 0x30);

/// Width of the settings pane, borders included.
const SETTINGS_W: u16 = 44;

fn draw_settings(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Settings;
    let title = match (app.section, app.selected_entry()) {
        (Section::Nightlight, _) => "nightlight".into(),
        (Section::LidClose, _) => "lid close".into(),
        (Section::Displays, Some(e)) => format!("settings: {} {}", e.number, e.live.name),
        (Section::Displays, None) => "settings".into(),
    };
    let mut block = boxed(&title, SETTINGS_BOX, focused);
    if app.section != Section::Displays && !app.daemon_running {
        block = block.title(daemon_warning());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);

    const LABEL_W: usize = 14;
    let current = app.current_setting();
    let mut rows = Vec::new();
    for (i, setting) in app.settings().into_iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let row = Rect {
            y: inner.y + i as u16,
            height: 1,
            ..inner
        };
        let (label, value, active) = setting_text(app, setting);
        let mut label_style = Style::new().fg(if active { FG } else { INACTIVE });
        let mut value_style = Style::new().fg(if active { TITLE } else { INACTIVE });
        if setting == current {
            let bg = if focused { SELECTED_BG } else { UNFOCUSED_BG };
            label_style = label_style.bg(bg);
            value_style = value_style.bg(bg).add_modifier(Modifier::BOLD);
        }
        let label = format!(" {label:<LABEL_W$}");
        let value = pad_to(&value, row.width.saturating_sub(LABEL_W as u16 + 1));
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label, label_style),
                Span::styled(value, value_style),
            ])),
            row,
        );
        rows.push((setting, row));
    }
    app.hits.setting_rows = rows;
}

fn daemon_warning() -> Line<'static> {
    Line::styled(" daemon not running ", Style::new().fg(WARN)).right_aligned()
}

fn draw_options(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Options;
    let setting = app.current_setting();
    let (label, _, active) = setting_text(app, setting);
    let title = match app.section {
        Section::LidClose => label.to_lowercase(),
        _ => format!("options: {}", label.to_lowercase()),
    };
    let mut block = boxed(&title, OPTIONS_BOX, focused);
    if app.section == Section::LidClose && !app.daemon_running {
        block = block.title(daemon_warning());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let area = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    match setting {
        Setting::Brightness => {
            let Some(p) = app.brightness().filter(|_| active) else {
                return;
            };
            draw_slider(
                f,
                app,
                area,
                p as f64 / 100.0,
                format!("{p}%"),
                "brighter",
                "dimmer",
            );
        }
        Setting::Temperature => {
            let k = app.nightlight.temperature;
            let fraction = app.temperature_fraction();
            draw_slider(f, app, area, fraction, format!("{k} K"), "cooler", "warmer");
        }
        Setting::ScheduleStart | Setting::ScheduleEnd => draw_time_picker(f, app, area),
        _ => draw_choices(f, app, area),
    }
}

/// Colours of the time picker's values by distance from the current one.
const TIME_FADE: [Color; 3] = [
    TITLE,
    Color::Rgb(0x8a, 0x8a, 0x8a),
    Color::Rgb(0x4a, 0x4a, 0x4a),
];

/// Hour and minute columns with the current time centred and its
/// neighbours fading out above and below.
fn draw_time_picker(f: &mut Frame, app: &mut App, area: Rect) {
    const CELL_W: u16 = 4;
    if area.width < CELL_W * 2 + 1 || area.height == 0 {
        return;
    }
    let t = app.picker_time();
    let focused = app.focus == Focus::Options;
    let mid = area.y + (area.height - 1) / 2;
    let mut hits = Vec::new();
    let columns = [
        (TimeColumn::Hour, area.x, t.hour()),
        (TimeColumn::Minute, area.x + CELL_W + 1, t.minute()),
    ];
    for (column, x, current) in columns {
        for offset in -2i32..=2 {
            let y = mid as i32 + offset;
            if y < area.y as i32 || y >= area.bottom() as i32 {
                continue;
            }
            let value = match column {
                TimeColumn::Hour => app::step_hour(current, offset),
                TimeColumn::Minute => app::step_minute(current, offset),
            };
            let mut style = Style::new().fg(TIME_FADE[offset.unsigned_abs() as usize]);
            if offset == 0 {
                let bg = if focused && app.time_column == column {
                    SELECTED_BG
                } else {
                    UNFOCUSED_BG
                };
                style = style.bg(bg).add_modifier(Modifier::BOLD);
            }
            let cell = Rect::new(x, y as u16, CELL_W, 1);
            f.render_widget(
                Paragraph::new(Span::styled(format!(" {value:02} "), style)),
                cell,
            );
            hits.push((column, value, cell));
        }
    }
    let colon = Span::styled(":", Style::new().fg(TITLE).add_modifier(Modifier::BOLD));
    f.render_widget(Paragraph::new(colon), Rect::new(area.x + CELL_W, mid, 1, 1));
    app.hits.times = hits;
}

/// A vertical slider on the left of the pane, filled from the bottom in
/// half-row steps, with the value and a key hint beside it.
fn draw_slider(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    fraction: f64,
    value: String,
    up: &str,
    down: &str,
) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    let focused = app.focus == Focus::Options;
    let rows = area.height;
    let halves = (fraction.clamp(0.0, 1.0) * rows as f64 * 2.0).round() as u32;
    let fill = Style::new().fg(if focused { HI } else { TITLE });
    let empty = Style::new().fg(INACTIVE);
    let bar: Vec<Line> = (0..rows)
        .map(|y| {
            // Half-row units below the top of this row (counted from the bottom).
            let below = (rows - 1 - y) as u32 * 2;
            match halves.saturating_sub(below) {
                0 => Line::styled("│", empty),
                1 => Line::styled("╻", fill),
                _ => Line::styled("┃", fill),
            }
        })
        .collect();
    f.render_widget(Paragraph::new(bar), Rect { width: 1, ..area });

    let text_x = area.x + 3;
    let text_w = area.width.saturating_sub(3);
    let mid = area.y + rows.saturating_sub(1) / 2;
    let value = Span::styled(value, Style::new().fg(TITLE).add_modifier(Modifier::BOLD));
    f.render_widget(Paragraph::new(value), Rect::new(text_x, mid, text_w, 1));
    if mid + 2 < area.bottom() {
        let key = Style::new().fg(HI).add_modifier(Modifier::BOLD);
        let hint = Line::from(vec![
            Span::styled("↑", key),
            Span::styled(format!(" {up}  "), Style::new().fg(INACTIVE)),
            Span::styled("↓", key),
            Span::styled(
                format!(" {down}  · applies immediately"),
                Style::new().fg(INACTIVE),
            ),
        ]);
        f.render_widget(Paragraph::new(hint), Rect::new(text_x, mid + 2, text_w, 1));
    }
    // Clickable area: the bar plus a column either side, so it's easy to hit.
    let x = area.x.saturating_sub(1);
    app.hits.slider = Some(Rect::new(x, area.y, area.x + 2 - x, rows));
}

/// Label, current value, and whether the setting is currently adjustable.
fn setting_text(app: &App, s: Setting) -> (&'static str, String, bool) {
    let n = &app.nightlight;
    match s {
        Setting::NightlightEnabled => return ("Nightlight", on_off(n.enabled), true),
        Setting::Temperature => return ("Temperature", format!("{} K", n.temperature), true),
        Setting::Schedule => return ("Schedule", on_off(n.schedule), true),
        Setting::ScheduleStart => return ("Turn on at", n.start.to_string(), n.schedule),
        Setting::ScheduleEnd => return ("Turn off at", n.end.to_string(), n.schedule),
        Setting::LidClose => return ("Lid close", app.lid_close.lid_close.label().into(), true),
        _ => {}
    }
    let Some(e) = app.selected_entry() else {
        return ("", String::new(), false);
    };
    let on = e.pending.enabled;
    let mode = e.pending.mode.filter(|_| on);
    let active = on || s == Setting::Enabled;
    let value = match s {
        Setting::Enabled => Some(Choice::Enabled(on)),
        Setting::Brightness => {
            let value = app
                .brightness()
                .filter(|_| on)
                .map_or_else(|| "—".into(), |p| format!("{p}%"));
            return ("Brightness", value, active);
        }
        Setting::Resolution => mode.map(|m| Choice::Resolution((m.width, m.height))),
        Setting::Refresh => mode.map(Choice::Refresh),
        Setting::Orientation => on.then_some(Choice::Orientation(e.pending.transform)),
        Setting::Scale => on.then_some(Choice::Scale(e.pending.scale)),
        Setting::Duplicate => {
            let source = app.group_of(app.selected).map(|s| app.entries[s].number);
            let value = if on {
                app.choice_label(&Choice::Duplicate(source), e, false)
            } else {
                "—".into()
            };
            return ("Duplicate", value, active);
        }
        _ => None,
    };
    // A duplicate's scale follows from its source.
    let active = active && !(s == Setting::Scale && e.duplicates.is_some());
    let label = match s {
        Setting::Enabled => "Display",
        Setting::Resolution => "Resolution",
        Setting::Refresh => "Refresh rate",
        Setting::Orientation => "Orientation",
        _ => "Scale",
    };
    (
        label,
        value.map_or_else(|| "—".into(), |c| c.label(e, false)),
        active,
    )
}

fn on_off(on: bool) -> String {
    if on { "On" } else { "Off" }.into()
}

/// All choices for the focused setting, laid out in columns that flow top
/// to bottom. Scrolls horizontally to keep the current choice visible.
fn draw_choices(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(e) = app.selected_entry() else {
        return;
    };
    let (choices, current) = app.choices();
    if choices.is_empty() || area.width < 4 || area.height == 0 {
        return;
    }
    let labels: Vec<String> = choices
        .iter()
        .map(|c| app.choice_label(c, e, true))
        .collect();
    let col_w = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16 + 4;
    let rows = area.height as usize;
    let total_cols = labels.len().div_ceil(rows);
    let visible_cols = ((area.width / col_w) as usize).max(1);
    let cur_col = current.unwrap_or(0) / rows;
    let first_col = (cur_col + 1).saturating_sub(visible_cols);

    let mut hits = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let col = i / rows;
        if col < first_col || col >= first_col + visible_cols {
            continue;
        }
        let x = area.x + ((col - first_col) as u16) * col_w;
        let w = col_w.min(area.right() - x);
        let cell = Rect::new(x, area.y + (i % rows) as u16, w, 1);
        let fg = if choices[i].is_recommended(e) { HI } else { FG };
        let style = if Some(i) == current {
            let bg = if app.focus == Focus::Options {
                SELECTED_BG
            } else {
                UNFOCUSED_BG
            };
            Style::new().fg(TITLE).bg(bg).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(fg)
        };
        let text = pad_to(&format!(" {label}"), w.saturating_sub(1));
        f.render_widget(Paragraph::new(Span::styled(text, style)), cell);
        hits.push((i, cell));
    }
    // Arrows hint that more choices are scrolled out of view.
    let more = Style::new().fg(HI);
    if first_col > 0 {
        f.render_widget(
            Paragraph::new(Span::styled("‹", more)),
            Rect::new(area.x, area.y, 1, 1),
        );
    }
    if first_col + visible_cols < total_cols {
        let r = Rect::new(area.right() - 1, area.y, 1, 1);
        f.render_widget(Paragraph::new(Span::styled("›", more)), r);
    }
    app.hits.choices = hits;
}

/// A button reading ` label `, its first letter (its key) picked out
/// unless the button is `dim`, and its width.
fn button(label: &'static str, style: Style, dim: bool) -> (Vec<Span<'static>>, u16) {
    let (first, rest) = label.split_at(1);
    let first_style = if dim {
        style
    } else {
        style.fg(KEY).add_modifier(Modifier::BOLD)
    };
    let spans = vec![
        Span::styled(" ", style),
        Span::styled(first, first_style),
        Span::styled(format!("{rest} "), style),
    ];
    (spans, label.len() as u16 + 2)
}

fn draw_footer(f: &mut Frame, app: &mut App, area: Rect) {
    let dirty = app.is_dirty();
    let text = Style::new().fg(FG);
    let dim = Style::new().fg(INACTIVE);
    let apply_style = if dirty {
        Style::new()
            .fg(TITLE)
            .bg(SELECTED_BG)
            .add_modifier(Modifier::BOLD)
    } else {
        dim
    };
    let reset_style = if dirty { text } else { dim };

    let buttons = [
        ("apply", apply_style, !dirty),
        ("reset", reset_style, !dirty),
        ("identify", text, false),
        ("quit", text, false),
    ];
    let mut spans = Vec::new();
    let mut hits = Vec::new();
    let mut x = area.x;
    for (label, style, dim) in buttons {
        let (b, w) = button(label, style, dim);
        spans.push(Span::raw(" "));
        spans.extend(b);
        hits.push(Rect {
            x: x + 1,
            width: w,
            ..area
        });
        x += w + 1;
    }
    app.hits.apply = Some(hits[0]);
    app.hits.reset = Some(hits[1]);
    app.hits.identify = Some(hits[2]);
    app.hits.quit = Some(hits[3]);
    f.render_widget(Paragraph::new(Line::from(spans)), area);

    if let Some(msg) = &app.status {
        let w = (msg.chars().count() as u16 + 1).min(area.width);
        let r = Rect {
            x: area.right() - w,
            width: w,
            ..area
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg.as_str(), Style::new().fg(WARN))),
            r,
        );
    }
}

fn draw_confirm(f: &mut Frame, app: &mut App) {
    let Some(c) = &app.confirm else { return };
    let secs = c
        .deadline
        .saturating_duration_since(Instant::now())
        .as_secs_f64()
        .ceil() as u64;
    let area = f.area();
    let (w, h) = (44u16.min(area.width), 6u16.min(area.height));
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, r);
    let block = boxed("confirm", HI, true);
    let inner = block.inner(r);
    f.render_widget(block, r);

    let lines = vec![
        Line::styled(
            "Keep these display settings?",
            Style::new().fg(TITLE).add_modifier(Modifier::BOLD),
        ),
        Line::styled(format!("Reverting in {secs} s"), Style::new().fg(WARN)),
        Line::raw(""),
        Line::from(
            [
                button(
                    "keep",
                    Style::new()
                        .fg(TITLE)
                        .bg(SELECTED_BG)
                        .add_modifier(Modifier::BOLD),
                    false,
                )
                .0,
                vec![Span::raw("    ")],
                button(
                    "revert",
                    Style::new().fg(FG).bg(Color::Rgb(0x30, 0x30, 0x30)),
                    false,
                )
                .0,
            ]
            .concat(),
        ),
    ];
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);

    // Buttons line is centred: " keep " (6) + 4 spaces + " revert " (8) = 18.
    if inner.height >= 4 {
        let y = inner.y + 3;
        let x0 = inner.x + inner.width.saturating_sub(18) / 2;
        app.hits.keep = Some(Rect::new(x0, y, 6, 1));
        app.hits.revert = Some(Rect::new(x0 + 10, y, 8, 1));
    }
}

fn pad_to(s: &str, width: u16) -> String {
    let n = s.chars().count();
    let w = width as usize;
    if n >= w {
        s.chars().take(w).collect()
    } else {
        format!("{s}{}", " ".repeat(w - n))
    }
}
