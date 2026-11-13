use std::os::fd::BorrowedFd;

use rsds_core::{Backend, Error, Mode, Output, OutputConfig, Transform};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
    backend::WaylandError,
    event_created_child,
    globals::{GlobalList, GlobalListContents, registry_queue_init},
    protocol::{wl_output, wl_registry},
};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

const MANAGER_VERSION: u32 = 4;

struct ModeState {
    proxy: ZwlrOutputModeV1,
    mode: Mode,
}

struct HeadState {
    proxy: ZwlrOutputHeadV1,
    /// Output being built up from head events. `modes` and `current_mode`
    /// are filled in from `mode_proxies` when the manager sends `done`.
    output: Output,
    mode_proxies: Vec<ZwlrOutputModeV1>,
    current_mode_proxy: Option<ZwlrOutputModeV1>,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) identify: crate::identify::Identify,
    pub(crate) gamma: crate::gamma::Gamma,
    heads: Vec<HeadState>,
    modes: Vec<ModeState>,
    /// Snapshot of `heads` taken at the last `done` event.
    outputs: Vec<Output>,
    serial: Option<u32>,
    changed: bool,
    finished: bool,
    apply_result: Option<Result<(), Error>>,
}

impl State {
    fn head(&mut self, proxy: &ZwlrOutputHeadV1) -> Option<&mut HeadState> {
        self.heads.iter_mut().find(|h| &h.proxy == proxy)
    }

    fn mode(&mut self, proxy: &ZwlrOutputModeV1) -> Option<&mut ModeState> {
        self.modes.iter_mut().find(|m| &m.proxy == proxy)
    }

    fn snapshot(&mut self) {
        let modes = &self.modes;
        let lookup = |p: &ZwlrOutputModeV1| modes.iter().find(|m| &m.proxy == p).map(|m| m.mode);
        self.outputs = self
            .heads
            .iter()
            .map(|h| {
                let mut out = h.output.clone();
                out.modes = h.mode_proxies.iter().filter_map(lookup).collect();
                out.current_mode = h
                    .current_mode_proxy
                    .as_ref()
                    .and_then(|c| h.mode_proxies.iter().position(|p| p == c));
                out
            })
            .collect();
        self.changed = true;
    }
}

/// Connection to a wlroots compositor's output manager.
pub struct WaylandBackend {
    /// Kept so `fd()` can borrow from it.
    wl_backend: wayland_client::backend::Backend,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    manager: ZwlrOutputManagerV1,
    globals: GlobalList,
    state: State,
}

impl WaylandBackend {
    pub fn connect() -> Result<Self, Error> {
        let conn = Connection::connect_to_env().map_err(|e| Error::Connection(e.to_string()))?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| Error::Connection(e.to_string()))?;
        let qh = queue.handle();
        let manager: ZwlrOutputManagerV1 = globals
            .bind(&qh, 1..=MANAGER_VERSION, ())
            .map_err(|_| Error::Unsupported("zwlr_output_manager_v1 (not a wlroots compositor?)".into()))?;

        let mut state = State::default();
        while state.serial.is_none() {
            queue
                .blocking_dispatch(&mut state)
                .map_err(|e| Error::Connection(e.to_string()))?;
        }
        state.changed = false;
        Ok(Self { wl_backend: conn.backend(), queue, qh, manager, globals, state })
    }

    fn connection_error(e: impl std::fmt::Display) -> Error {
        Error::Connection(e.to_string())
    }
}

impl Backend for WaylandBackend {
    fn fd(&self) -> BorrowedFd<'_> {
        self.wl_backend.poll_fd()
    }

    fn dispatch(&mut self) -> Result<bool, Error> {
        self.queue.flush().map_err(Self::connection_error)?;
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(Self::connection_error(e)),
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(Self::connection_error)?;
        // Event handlers may have queued replies (e.g. acking a configure and
        // attaching a buffer); send them now rather than at the next wakeup.
        self.queue.flush().map_err(Self::connection_error)?;
        if self.state.finished {
            return Err(Error::Connection("output manager finished".into()));
        }
        Ok(std::mem::take(&mut self.state.changed))
    }

    fn outputs(&self) -> &[Output] {
        &self.state.outputs
    }

    fn apply(&mut self, config: &[OutputConfig]) -> Result<(), Error> {
        self.submit(config, false)
    }

    fn test(&mut self, config: &[OutputConfig]) -> Result<(), Error> {
        self.submit(config, true)
    }

    fn identify(&mut self, labels: &[(String, usize)]) -> Result<(), Error> {
        crate::identify::show(&mut self.state, &mut self.queue, &self.qh, &self.globals, labels)
    }

    fn end_identify(&mut self) {
        crate::identify::hide(&mut self.state);
        let _ = self.queue.flush();
    }

    fn set_color_temperature(&mut self, kelvin: Option<u32>) -> Result<(), Error> {
        crate::gamma::set(&mut self.state, &self.qh, &self.globals, kelvin)?;
        self.queue.flush().map_err(Self::connection_error)
    }
}

impl WaylandBackend {
    /// Send a configuration to the compositor, either applying it or only
    /// asking whether it would be accepted.
    fn submit(&mut self, config: &[OutputConfig], test_only: bool) -> Result<(), Error> {
        for c in config {
            if !self.state.outputs.iter().any(|o| o.name == c.name) {
                return Err(Error::NotFound(c.name.clone()));
            }
        }
        let serial = self.state.serial.ok_or(Error::Cancelled)?;
        let conf = self.manager.create_configuration(serial, &self.qh, ());

        // The protocol requires every head to be mentioned; heads without a
        // requested config keep their current state.
        for (head, current) in self.state.heads.iter().zip(&self.state.outputs) {
            let wanted = config
                .iter()
                .find(|c| c.name == current.name)
                .cloned()
                .unwrap_or_else(|| current.config());

            if !wanted.enabled {
                conf.disable_head(&head.proxy);
                continue;
            }
            let ch = conf.enable_head(&head.proxy, &self.qh, ());
            if let Some(mode) = wanted.mode {
                let proxy = head.mode_proxies.iter().find(|p| {
                    self.state
                        .modes
                        .iter()
                        .any(|m| &m.proxy == *p && m.mode.same_as(&mode))
                });
                match proxy {
                    Some(p) => ch.set_mode(p),
                    None => ch.set_custom_mode(mode.width, mode.height, mode.refresh_mhz),
                }
            }
            ch.set_position(wanted.position.0, wanted.position.1);
            ch.set_transform(to_wl_transform(wanted.transform));
            ch.set_scale(wanted.scale);
        }
        if test_only {
            conf.test();
        } else {
            conf.apply();
        }

        self.state.apply_result = None;
        let result = loop {
            if let Some(r) = self.state.apply_result.take() {
                break r;
            }
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(Self::connection_error)?;
        };
        conf.destroy();
        result
    }
}

fn from_wl_transform(t: WEnum<wl_output::Transform>) -> Transform {
    use wl_output::Transform as W;
    match t {
        WEnum::Value(W::_90) => Transform::Rotate90,
        WEnum::Value(W::_180) => Transform::Rotate180,
        WEnum::Value(W::_270) => Transform::Rotate270,
        WEnum::Value(W::Flipped) => Transform::Flipped,
        WEnum::Value(W::Flipped90) => Transform::Flipped90,
        WEnum::Value(W::Flipped180) => Transform::Flipped180,
        WEnum::Value(W::Flipped270) => Transform::Flipped270,
        _ => Transform::Normal,
    }
}

fn to_wl_transform(t: Transform) -> wl_output::Transform {
    use wl_output::Transform as W;
    match t {
        Transform::Normal => W::Normal,
        Transform::Rotate90 => W::_90,
        Transform::Rotate180 => W::_180,
        Transform::Rotate270 => W::_270,
        Transform::Flipped => W::Flipped,
        Transform::Flipped90 => W::Flipped90,
        Transform::Flipped180 => W::Flipped180,
        Transform::Flipped270 => W::Flipped270,
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        crate::gamma::registry_event(state, registry, event, qh);
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_manager_v1::Event;
        match event {
            Event::Head { head } => state.heads.push(HeadState {
                proxy: head,
                output: Output {
                    name: String::new(),
                    make: String::new(),
                    model: String::new(),
                    serial: None,
                    physical_size_mm: None,
                    modes: Vec::new(),
                    enabled: false,
                    current_mode: None,
                    position: (0, 0),
                    transform: Transform::Normal,
                    scale: 1.0,
                },
                mode_proxies: Vec::new(),
                current_mode_proxy: None,
            }),
            Event::Done { serial } => {
                state.serial = Some(serial);
                state.snapshot();
            }
            Event::Finished => state.finished = true,
            _ => {}
        }
    }

    event_created_child!(State, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_head_v1::Event;
        if let Event::Finished = event {
            state.heads.retain(|h| &h.proxy != proxy);
            if proxy.version() >= 3 {
                proxy.release();
            }
            return;
        }
        if let Event::Mode { mode } = event {
            state.modes.push(ModeState {
                proxy: mode.clone(),
                mode: Mode { width: 0, height: 0, refresh_mhz: 0, preferred: false },
            });
            if let Some(h) = state.head(proxy) {
                h.mode_proxies.push(mode);
            }
            return;
        }
        let Some(h) = state.head(proxy) else { return };
        let o = &mut h.output;
        match event {
            Event::Name { name } => o.name = name,
            Event::PhysicalSize { width, height } => {
                o.physical_size_mm = (width > 0 && height > 0).then_some((width, height));
            }
            Event::Enabled { enabled } => {
                o.enabled = enabled != 0;
                if !o.enabled {
                    h.current_mode_proxy = None;
                }
            }
            Event::CurrentMode { mode } => h.current_mode_proxy = Some(mode),
            Event::Position { x, y } => o.position = (x, y),
            Event::Transform { transform } => o.transform = from_wl_transform(transform),
            Event::Scale { scale } => o.scale = scale,
            Event::Make { make } => o.make = make,
            Event::Model { model } => o.model = model,
            Event::SerialNumber { serial_number } => {
                o.serial = (!serial_number.is_empty()).then_some(serial_number);
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_mode_v1::Event;
        if let Event::Finished = event {
            state.modes.retain(|m| &m.proxy != proxy);
            for h in &mut state.heads {
                h.mode_proxies.retain(|p| p != proxy);
                if h.current_mode_proxy.as_ref() == Some(proxy) {
                    h.current_mode_proxy = None;
                }
            }
            if proxy.version() >= 3 {
                proxy.release();
            }
            return;
        }
        let Some(m) = state.mode(proxy) else { return };
        match event {
            Event::Size { width, height } => {
                m.mode.width = width;
                m.mode.height = height;
            }
            Event::Refresh { refresh } => m.mode.refresh_mhz = refresh,
            Event::Preferred => m.mode.preferred = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_configuration_v1::Event;
        state.apply_result = match event {
            Event::Succeeded => Some(Ok(())),
            Event::Failed => Some(Err(Error::Rejected)),
            Event::Cancelled => Some(Err(Error::Cancelled)),
            _ => None,
        };
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputConfigurationHeadV1,
        _: <ZwlrOutputConfigurationHeadV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
