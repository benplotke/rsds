//! X11 backend, using RandR 1.3+ over a pure-Rust X connection.

mod edid;
mod identify;

use std::os::fd::{AsFd, BorrowedFd};

use rsds_core::{
    Backend, Error, Mode, Output, OutputConfig, Transform,
    nightlight::{NEUTRAL_K, gamma_ramp},
};
use x11rb::{
    connection::{Connection, RequestConnection},
    errors::ReplyError,
    protocol::{
        Event,
        dpms::{self, ConnectionExt as _},
        randr::{self, ConnectionExt as _},
        render,
        xproto::{self, ConnectionExt as _},
    },
    rust_connection::RustConnection,
};

/// The X `None` resource and `CurrentTime`.
const NONE: u32 = 0;
/// 1.0 in the 16.16 fixed point of RandR transforms.
const ONE: i32 = 1 << 16;

/// A CRTC as it is now.
struct Crtc {
    id: randr::Crtc,
    info: randr::GetCrtcInfoReply,
    transform: [i32; 9],
}

/// X resources of a connected output, alongside its [`Output`].
struct Ids {
    output: randr::Output,
    /// Its CRTC, or NONE.
    crtc: randr::Crtc,
    possible: Vec<randr::Crtc>,
    /// The RandR mode of each of the output's modes.
    modes: Vec<randr::Mode>,
}

/// The server's configuration, as last read.
#[derive(Default)]
struct State {
    config_timestamp: xproto::Timestamp,
    outputs: Vec<Output>,
    /// Parallel to `outputs`.
    ids: Vec<Ids>,
    crtcs: Vec<Crtc>,
    primary: randr::Output,
    /// Screen (framebuffer) size in pixels.
    size: (u16, u16),
}

/// What one CRTC should be set to.
#[derive(Clone, PartialEq)]
struct CrtcSetting {
    x: i16,
    y: i16,
    mode: randr::Mode,
    rotation: u16,
    outputs: Vec<randr::Output>,
    transform: [i32; 9],
}

/// A configuration worked out for the server.
struct Plan {
    size: (u16, u16),
    /// Every CRTC whose setting changes, and to what (None: off).
    changes: Vec<(randr::Crtc, Option<CrtcSetting>)>,
    primary: randr::Output,
}

/// Connection to an X server with RandR.
pub struct X11Backend {
    conn: RustConnection,
    root: xproto::Window,
    edid_atom: xproto::Atom,
    /// Screen pixels per millimetre at startup, kept when the size changes.
    dpmm: (f64, f64),
    state: State,
    changed: bool,
    kelvin: Option<u32>,
    /// Active CRTCs and their modes when the tint was last applied.
    tinted: Vec<(randr::Crtc, randr::Mode)>,
    /// Whether DPMS was enabled before we switched the display off.
    dpms_was_enabled: Option<bool>,
    identify: identify::Identify,
}

fn lost(e: impl std::fmt::Display) -> Error {
    Error::Connection(e.to_string())
}

/// A failed request while configuring: the server refused it.
fn refused(e: ReplyError) -> Error {
    match e {
        ReplyError::ConnectionError(e) => lost(e),
        ReplyError::X11Error(_) => Error::Rejected,
    }
}

fn status(s: randr::SetConfig) -> Result<(), Error> {
    match s {
        randr::SetConfig::SUCCESS => Ok(()),
        randr::SetConfig::INVALID_CONFIG_TIME | randr::SetConfig::INVALID_TIME => Err(Error::Cancelled),
        _ => Err(Error::Rejected),
    }
}

impl X11Backend {
    pub fn connect() -> Result<Self, Error> {
        let (conn, screen) = x11rb::connect(None).map_err(lost)?;
        let s = &conn.setup().roots[screen];
        let root = s.root;
        let dpmm = (
            s.width_in_pixels as f64 / s.width_in_millimeters.max(1) as f64,
            s.height_in_pixels as f64 / s.height_in_millimeters.max(1) as f64,
        );
        if conn.extension_information(randr::X11_EXTENSION_NAME).map_err(lost)?.is_none() {
            return Err(Error::Unsupported("RandR extension".into()));
        }
        let v = conn.randr_query_version(1, 6).map_err(lost)?.reply().map_err(lost)?;
        if (v.major_version, v.minor_version) < (1, 3) {
            return Err(Error::Unsupported(format!("RandR {}.{} (1.3 needed)", v.major_version, v.minor_version)));
        }
        let mask = randr::NotifyMask::SCREEN_CHANGE | randr::NotifyMask::CRTC_CHANGE | randr::NotifyMask::OUTPUT_CHANGE;
        conn.randr_select_input(root, mask).map_err(lost)?;
        let edid_atom = conn.intern_atom(false, b"EDID").map_err(lost)?.reply().map_err(lost)?.atom;
        let mut backend = Self {
            conn,
            root,
            edid_atom,
            dpmm,
            state: State::default(),
            changed: false,
            kelvin: None,
            tinted: Vec::new(),
            dpms_was_enabled: None,
            identify: identify::Identify::default(),
        };
        backend.state = backend.query()?;
        Ok(backend)
    }

    /// Read the server's current configuration.
    fn query(&self) -> Result<State, Error> {
        let c = &self.conn;
        let res = c.randr_get_screen_resources_current(self.root).map_err(lost)?.reply().map_err(lost)?;
        let ts = res.config_timestamp;
        let primary = c.randr_get_output_primary(self.root).map_err(lost)?;
        let geometry = c.get_geometry(self.root).map_err(lost)?;
        let crtc_cookies = res
            .crtcs
            .iter()
            .map(|&id| Ok((id, c.randr_get_crtc_info(id, ts)?, c.randr_get_crtc_transform(id)?)))
            .collect::<Result<Vec<_>, x11rb::errors::ConnectionError>>()
            .map_err(lost)?;
        let output_cookies = res
            .outputs
            .iter()
            .map(|&id| c.randr_get_output_info(id, ts))
            .collect::<Result<Vec<_>, _>>()
            .map_err(lost)?;
        let primary = primary.reply().map_err(lost)?.output;
        let geometry = geometry.reply().map_err(lost)?;
        let crtcs = crtc_cookies
            .into_iter()
            .map(|(id, info, transform)| {
                Ok(Crtc { id, info: info.reply()?, transform: matrix(&transform.reply()?.current_transform) })
            })
            .collect::<Result<Vec<_>, ReplyError>>()
            .map_err(lost)?;
        let infos = output_cookies
            .into_iter()
            .map(|c| c.reply())
            .collect::<Result<Vec<_>, _>>()
            .map_err(lost)?;

        let mut state = State {
            config_timestamp: ts,
            primary,
            size: (geometry.width, geometry.height),
            ..State::default()
        };
        for (&id, info) in res.outputs.iter().zip(infos) {
            if info.connection != randr::Connection::CONNECTED {
                continue;
            }
            let name = String::from_utf8_lossy(&info.name).into_owned();
            let edid = self.edid(id).and_then(|e| edid::parse(&e));

            let mut modes: Vec<Mode> = Vec::new();
            let mut mode_ids = Vec::new();
            for (i, &mid) in info.modes.iter().enumerate() {
                let Some(m) = res.modes.iter().find(|m| m.id == mid) else { continue };
                let m = Mode { preferred: i < info.num_preferred as usize, ..mode(m) };
                // Modes differing only in sync flags or timings look the same here.
                if !modes.iter().any(|x| x.same_as(&m)) {
                    modes.push(m);
                    mode_ids.push(mid);
                }
            }

            let mut output = Output {
                name,
                make: edid.as_ref().map_or_else(String::new, |e| e.make.clone()),
                model: edid.as_ref().map_or_else(String::new, |e| e.model.clone()),
                serial: edid.and_then(|e| e.serial),
                modes,
                enabled: false,
                current_mode: None,
                position: (0, 0),
                transform: Transform::Normal,
                scale: 1.0,
                primary: id == primary,
            };
            if let Some(crtc) = crtcs.iter().find(|c| c.id == info.crtc && c.info.mode != NONE)
                && let Some(current) = res.modes.iter().find(|m| m.id == crtc.info.mode).map(mode) {
                    let index = output.modes.iter().position(|m| m.same_as(&current)).unwrap_or_else(|| {
                        output.modes.push(current);
                        mode_ids.push(crtc.info.mode);
                        output.modes.len() - 1
                    });
                    output.enabled = true;
                    output.current_mode = Some(index);
                    output.position = (crtc.info.x as i32, crtc.info.y as i32);
                    output.transform = from_rotation(u16::from(crtc.info.rotation));
                    output.scale = scale_of(&crtc.transform);
                }
            state.outputs.push(output);
            state.ids.push(Ids { output: id, crtc: info.crtc, possible: info.crtcs, modes: mode_ids });
        }
        state.crtcs = crtcs;
        Ok(state)
    }

    fn edid(&self, output: randr::Output) -> Option<Vec<u8>> {
        let reply = self
            .conn
            .randr_get_output_property(output, self.edid_atom, xproto::AtomEnum::ANY, 0, 256, false, false)
            .ok()?
            .reply()
            .ok()?;
        Some(reply.data)
    }

    /// Re-read the configuration, noting whether the outputs changed.
    fn refresh(&mut self) -> Result<(), Error> {
        let mut state = self.query()?;
        // X keeps a CRTC going when its monitor is unplugged; turn it off
        // (and shrink the screen), as a Wayland compositor would.
        let connected = |o: &randr::Output| state.ids.iter().any(|i| i.output == *o);
        if state.crtcs.iter().any(|c| c.info.mode != NONE && !c.info.outputs.iter().any(connected)) {
            let configs: Vec<OutputConfig> = state.outputs.iter().map(Output::config).collect();
            let previous = std::mem::replace(&mut self.state, state);
            if let Ok(plan) = self.plan(&configs) {
                let _ = self.execute(&plan);
            }
            self.state = previous;
            state = self.query()?;
        }
        if state.outputs != self.state.outputs {
            self.changed = true;
        }
        self.state = state;
        if self.kelvin.is_some() && self.active_crtcs() != self.tinted {
            self.apply_tint()?;
        }
        Ok(())
    }

    /// Handle events that have arrived, including ones read from the
    /// socket while waiting for replies (which won't make the fd readable
    /// again), so nothing is left waiting when the caller polls.
    fn settle(&mut self) -> Result<(), Error> {
        loop {
            let mut randr = false;
            while let Some(event) = self.conn.poll_for_event().map_err(lost)? {
                randr |= matches!(event, Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_));
            }
            if !randr {
                return Ok(());
            }
            self.refresh()?;
        }
    }

    fn active_crtcs(&self) -> Vec<(randr::Crtc, randr::Mode)> {
        self.state.crtcs.iter().filter(|c| c.info.mode != NONE).map(|c| (c.id, c.info.mode)).collect()
    }

    /// Work out how to put `config` into effect, or why it can't be.
    fn plan(&self, config: &[OutputConfig]) -> Result<Plan, Error> {
        let s = &self.state;
        for c in config {
            if !s.outputs.iter().any(|o| o.name == c.name) {
                return Err(Error::NotFound(c.name.clone()));
            }
        }
        let wanted: Vec<OutputConfig> = s
            .outputs
            .iter()
            .map(|o| config.iter().find(|c| c.name == o.name).cloned().unwrap_or_else(|| o.config()))
            .collect();

        // X can't place a monitor at a negative position: shift the layout.
        let on = || wanted.iter().filter(|w| w.enabled && w.mode.is_some());
        let dx = on().map(|w| w.position.0).min().unwrap_or(0);
        let dy = on().map(|w| w.position.1).min().unwrap_or(0);

        let mut settings: Vec<Option<CrtcSetting>> = vec![None; s.outputs.len()];
        let mut size = (0i64, 0i64);
        for (i, w) in wanted.iter().enumerate() {
            let Some(m) = w.mode.filter(|_| w.enabled) else { continue };
            let (o, ids) = (&s.outputs[i], &s.ids[i]);
            // Keep the RandR mode it has if it is the same one.
            let mode = match o.mode() {
                Some(cur) if cur.same_as(&m) => ids.modes[o.current_mode.unwrap_or(0)],
                _ => o
                    .modes
                    .iter()
                    .position(|x| x.same_as(&m))
                    .map(|i| ids.modes[i])
                    .ok_or_else(|| Error::NotFound(format!("{} mode {}x{}", w.name, m.width, m.height)))?,
            };
            // Rounded down, so the area shown isn't a pixel too big.
            let k = (ONE as f64 / w.scale.max(1.0 / 256.0)).floor() as i32;
            let (pw, ph) = if w.transform.is_rotated() { (m.height, m.width) } else { (m.width, m.height) };
            // The area of the screen it shows, as the server rounds it.
            let footprint = |px: i32| (px as i64 * k as i64 + 0xffff) >> 16;
            let (x, y) = (w.position.0 - dx, w.position.1 - dy);
            size.0 = size.0.max(x as i64 + footprint(pw));
            size.1 = size.1.max(y as i64 + footprint(ph));
            settings[i] = Some(CrtcSetting {
                x: x.try_into().map_err(|_| Error::Rejected)?,
                y: y.try_into().map_err(|_| Error::Rejected)?,
                mode,
                rotation: to_rotation(w.transform),
                outputs: vec![ids.output],
                transform: [k, 0, 0, 0, k, 0, 0, 0, ONE],
            });
        }
        let range = self
            .conn
            .randr_get_screen_size_range(self.root)
            .map_err(lost)?
            .reply()
            .map_err(lost)?;
        if size.0 > range.max_width as i64 || size.1 > range.max_height as i64 {
            return Err(Error::Rejected);
        }
        let size = (
            (size.0 as u16).max(range.min_width),
            (size.1 as u16).max(range.min_height),
        );

        // CRTCs: an output that is on keeps its own; others take a free one.
        let mut assigned: Vec<Option<randr::Crtc>> = vec![None; s.outputs.len()];
        for (i, ids) in s.ids.iter().enumerate() {
            let own = s.crtcs.iter().any(|c| c.id == ids.crtc && c.info.outputs == [ids.output]);
            if settings[i].is_some() && own {
                assigned[i] = Some(ids.crtc);
            }
        }
        for i in 0..s.outputs.len() {
            if settings[i].is_none() || assigned[i].is_some() {
                continue;
            }
            let free = s.ids[i].possible.iter().copied().find(|c| !assigned.contains(&Some(*c)));
            assigned[i] = Some(free.ok_or(Error::Rejected)?);
        }

        let changes = s
            .crtcs
            .iter()
            .filter_map(|c| {
                let target = assigned.iter().position(|a| *a == Some(c.id)).and_then(|i| settings[i].clone());
                let current = (c.info.mode != NONE).then(|| CrtcSetting {
                    x: c.info.x,
                    y: c.info.y,
                    mode: c.info.mode,
                    rotation: u16::from(c.info.rotation),
                    outputs: c.info.outputs.clone(),
                    transform: c.transform,
                });
                (target != current).then_some((c.id, target))
            })
            .collect();
        let primary = wanted
            .iter()
            .zip(&s.ids)
            .find(|(w, _)| w.primary && w.enabled)
            .map_or(NONE, |(_, ids)| ids.output);
        Ok(Plan { size, changes, primary })
    }

    /// Put a plan into effect, with the server grabbed so other clients
    /// don't see the steps in between.
    fn execute(&mut self, plan: &Plan) -> Result<(), Error> {
        self.conn.grab_server().map_err(lost)?;
        let result = self.execute_grabbed(plan);
        self.conn.ungrab_server().map_err(lost)?;
        self.conn.flush().map_err(lost)?;
        result
    }

    fn execute_grabbed(&mut self, plan: &Plan) -> Result<(), Error> {
        let c = &self.conn;
        let ts = self.state.config_timestamp;
        let (w, h) = (plan.size.0 as i32, plan.size.1 as i32);

        // Turn off CRTCs that go off, change outputs, or don't fit the new
        // screen size as they are, so the size can change.
        for (id, target) in &plan.changes {
            let Some(cur) = self.state.crtcs.iter().find(|c| c.id == *id) else { continue };
            let i = &cur.info;
            let outside = i.x as i32 + i.width as i32 > w || i.y as i32 + i.height as i32 > h;
            let outputs_change = target.as_ref().is_none_or(|t| t.outputs != i.outputs);
            if i.mode != NONE && (outside || outputs_change) {
                let r = c
                    .randr_set_crtc_config(*id, NONE, ts, 0, 0, NONE, randr::Rotation::ROTATE0, &[])
                    .map_err(lost)?
                    .reply()
                    .map_err(refused)?;
                status(r.status)?;
            }
        }
        if plan.size != self.state.size {
            let mm = |px: u16, dpmm: f64| (px as f64 / dpmm).round() as u32;
            c.randr_set_screen_size(
                self.root,
                plan.size.0,
                plan.size.1,
                mm(plan.size.0, self.dpmm.0),
                mm(plan.size.1, self.dpmm.1),
            )
            .map_err(lost)?
            .check()
            .map_err(refused)?;
        }
        for (id, target) in &plan.changes {
            let Some(t) = target else { continue };
            let current = self.state.crtcs.iter().find(|c| c.id == *id).map(|c| c.transform);
            if current != Some(t.transform) {
                // Takes effect with the next SetCrtcConfig.
                let filter: &[u8] = if t.transform[0] == ONE { b"nearest" } else { b"bilinear" };
                c.randr_set_crtc_transform(*id, to_render(&t.transform), filter, &[])
                    .map_err(lost)?
                    .check()
                    .map_err(refused)?;
            }
            let r = c
                .randr_set_crtc_config(*id, NONE, ts, t.x, t.y, t.mode, t.rotation.into(), &t.outputs)
                .map_err(lost)?
                .reply()
                .map_err(refused)?;
            status(r.status)?;
        }
        if plan.primary != self.state.primary {
            c.randr_set_output_primary(self.root, plan.primary)
                .map_err(lost)?
                .check()
                .map_err(refused)?;
        }
        Ok(())
    }

    fn try_apply(&mut self, config: &[OutputConfig]) -> Result<(), Error> {
        self.refresh()?;
        let plan = self.plan(config)?;
        self.execute(&plan)
    }

    /// Set the Nightlight gamma ramp on every active CRTC.
    fn apply_tint(&mut self) -> Result<(), Error> {
        let kelvin = self.kelvin.unwrap_or(NEUTRAL_K);
        let crtcs = self.active_crtcs();
        for &(crtc, _) in &crtcs {
            let size = self
                .conn
                .randr_get_crtc_gamma_size(crtc)
                .map_err(lost)?
                .reply()
                .map_err(lost)?
                .size as usize;
            if size == 0 {
                continue;
            }
            let ramp = gamma_ramp(size, kelvin);
            let (r, gb) = ramp.split_at(size);
            let (g, b) = gb.split_at(size);
            self.conn.randr_set_crtc_gamma(crtc, r, g, b).map_err(lost)?;
        }
        self.conn.flush().map_err(lost)?;
        self.tinted = crtcs;
        Ok(())
    }
}

impl Backend for X11Backend {
    fn fd(&self) -> BorrowedFd<'_> {
        self.conn.stream().as_fd()
    }

    fn dispatch(&mut self) -> Result<bool, Error> {
        self.settle()?;
        Ok(std::mem::take(&mut self.changed))
    }

    fn has_primary(&self) -> bool {
        true
    }

    fn true_scaling(&self) -> bool {
        false
    }

    fn outputs(&self) -> &[Output] {
        &self.state.outputs
    }

    /// Unlike a Wayland compositor, X takes a configuration a CRTC at a
    /// time; if a step is refused, the original configuration is restored.
    fn apply(&mut self, config: &[OutputConfig]) -> Result<(), Error> {
        let original: Vec<OutputConfig> = self.state.outputs.iter().map(Output::config).collect();
        let result = self.try_apply(config);
        if let Err(Error::Rejected) = result {
            let _ = self.try_apply(&original);
        }
        self.refresh()?;
        self.settle()?;
        result
    }

    /// X has no way to test a configuration; this checks what can be known
    /// beforehand (modes, CRTCs, screen size).
    fn test(&mut self, config: &[OutputConfig]) -> Result<(), Error> {
        self.plan(config).map(|_| ())
    }

    fn identify(&mut self, labels: &[(String, usize)]) -> Result<(), Error> {
        let result = self.identify.show(&self.conn, self.root, &self.state.outputs, labels);
        self.settle()?;
        result
    }

    fn end_identify(&mut self) {
        self.identify.hide(&self.conn);
        let _ = self.settle();
    }

    fn set_color_temperature(&mut self, kelvin: Option<u32>) -> Result<(), Error> {
        let was = self.kelvin;
        self.kelvin = kelvin;
        // Without a tint, leave the ramps alone unless we tinted them.
        let result = if kelvin.is_some() || was.is_some() { self.apply_tint() } else { Ok(()) };
        self.settle()?;
        result
    }

    /// X can only switch all displays off (DPMS), and any input switches
    /// them back on. This is used for the built-in panel when it is the
    /// only display, so that is what goes off.
    fn set_power(&mut self, output: &str, on: bool) -> Result<(), Error> {
        if !self.state.outputs.iter().any(|o| o.name == output) {
            return Err(Error::NotFound(output.into()));
        }
        let c = &self.conn;
        if c.extension_information(dpms::X11_EXTENSION_NAME).map_err(lost)?.is_none() {
            return Err(Error::Unsupported("DPMS extension".into()));
        }
        if on {
            c.dpms_force_level(dpms::DPMSMode::ON).map_err(lost)?.check().map_err(refused)?;
            if self.dpms_was_enabled.take() == Some(false) {
                c.dpms_disable().map_err(lost)?.check().map_err(refused)?;
            }
        } else {
            let enabled = c.dpms_info().map_err(lost)?.reply().map_err(lost)?.state;
            if !enabled {
                // Forcing a level needs DPMS enabled.
                c.dpms_enable().map_err(lost)?.check().map_err(refused)?;
            }
            self.dpms_was_enabled.get_or_insert(enabled);
            c.dpms_force_level(dpms::DPMSMode::OFF).map_err(lost)?.check().map_err(refused)?;
        }
        self.settle()
    }
}

/// A RandR mode, with its refresh rate worked out as xrandr does.
fn mode(m: &randr::ModeInfo) -> Mode {
    let flags = u32::from(m.mode_flags);
    let mut vtotal = m.vtotal as f64;
    if flags & u32::from(randr::ModeFlag::DOUBLE_SCAN) != 0 {
        vtotal *= 2.0;
    }
    if flags & u32::from(randr::ModeFlag::INTERLACE) != 0 {
        vtotal /= 2.0;
    }
    let refresh_mhz = if m.htotal > 0 && vtotal > 0.0 {
        (m.dot_clock as f64 * 1000.0 / (m.htotal as f64 * vtotal)).round() as i32
    } else {
        0
    };
    Mode { width: m.width as i32, height: m.height as i32, refresh_mhz, preferred: false }
}

fn matrix(t: &render::Transform) -> [i32; 9] {
    [
        t.matrix11, t.matrix12, t.matrix13, t.matrix21, t.matrix22, t.matrix23, t.matrix31, t.matrix32,
        t.matrix33,
    ]
}

fn to_render(m: &[i32; 9]) -> render::Transform {
    render::Transform {
        matrix11: m[0],
        matrix12: m[1],
        matrix13: m[2],
        matrix21: m[3],
        matrix22: m[4],
        matrix23: m[5],
        matrix31: m[6],
        matrix32: m[7],
        matrix33: m[8],
    }
}

/// Scale (logical size = mode / scale) from a CRTC transform, which maps
/// the monitor's pixels to the screen's, rounded to 1/256.
fn scale_of(m: &[i32; 9]) -> f64 {
    let k = (m[0] as f64).hypot(m[3] as f64) / ONE as f64;
    if k <= 0.0 || m[8] != ONE {
        return 1.0;
    }
    (256.0 / k).round() / 256.0
}

fn from_rotation(r: u16) -> Transform {
    use randr::Rotation as R;
    let has = |f: R| r & u16::from(f) != 0;
    let flipped = has(R::REFLECT_X);
    let flipped_y = has(R::REFLECT_Y);
    // A flip on y is a flip on x turned upside down.
    let quarter = if has(R::ROTATE90) {
        1
    } else if has(R::ROTATE180) {
        2
    } else if has(R::ROTATE270) {
        3
    } else {
        0
    };
    let (flipped, quarter) = match (flipped, flipped_y) {
        (true, true) => (false, (quarter + 2) % 4),
        (false, true) => (true, (quarter + 2) % 4),
        (f, false) => (f, quarter),
    };
    match (flipped, quarter) {
        (false, 0) => Transform::Normal,
        (false, 1) => Transform::Rotate90,
        (false, 2) => Transform::Rotate180,
        (false, _) => Transform::Rotate270,
        (true, 0) => Transform::Flipped,
        (true, 1) => Transform::Flipped90,
        (true, 2) => Transform::Flipped180,
        (true, _) => Transform::Flipped270,
    }
}

fn to_rotation(t: Transform) -> u16 {
    use randr::Rotation as R;
    let r = match t {
        Transform::Normal => R::ROTATE0,
        Transform::Rotate90 => R::ROTATE90,
        Transform::Rotate180 => R::ROTATE180,
        Transform::Rotate270 => R::ROTATE270,
        Transform::Flipped => R::ROTATE0 | R::REFLECT_X,
        Transform::Flipped90 => R::ROTATE90 | R::REFLECT_X,
        Transform::Flipped180 => R::ROTATE180 | R::REFLECT_X,
        Transform::Flipped270 => R::ROTATE270 | R::REFLECT_X,
    };
    r.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_round_trip() {
        use Transform::*;
        for t in [Normal, Rotate90, Rotate180, Rotate270, Flipped, Flipped90, Flipped180, Flipped270] {
            assert_eq!(from_rotation(to_rotation(t)), t);
        }
    }

    #[test]
    fn scale_from_transform() {
        let k = |s: f64| (ONE as f64 / s).floor() as i32;
        for s in [1.0, 1.25, 1.5, 1.75, 2.0, 341.0 / 256.0] {
            assert_eq!(scale_of(&[k(s), 0, 0, 0, k(s), 0, 0, 0, ONE]), s);
        }
    }

    #[test]
    fn refresh_like_xrandr() {
        // A 1920x1080 panel's mode; xrandr says 60.01 Hz.
        let m = randr::ModeInfo {
            width: 1920,
            height: 1080,
            dot_clock: 152_840_000,
            htotal: 2250,
            vtotal: 1132,
            ..Default::default()
        };
        assert_eq!(mode(&m).refresh_mhz, 60008);
    }
}
