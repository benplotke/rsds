//! Identify: a number badge on each output, drawn as an overlay
//! layer-shell surface backed by a shared-memory buffer.

use std::{io::Write, os::fd::AsFd};

use rsds_core::{
    Error,
    badge::{self, Badge},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    globals::GlobalList,
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_output::{self, WlOutput},
        wl_region::WlRegion,
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
};

use crate::output_management::State;

struct Globals {
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
}

struct BadgeSurface {
    number: usize,
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    buffer: Option<(WlShmPool, WlBuffer)>,
}

impl BadgeSurface {
    fn destroy(self) {
        self.layer.destroy();
        self.surface.destroy();
        if let Some((pool, buffer)) = self.buffer {
            buffer.destroy();
            pool.destroy();
        }
    }
}

/// Identify state. Globals are bound on first use only, so a process that
/// never identifies (the daemon) never binds them.
#[derive(Default)]
pub(crate) struct Identify {
    globals: Option<Globals>,
    /// wl_outputs bound while badges are shown, with their connector names.
    outputs: Vec<(WlOutput, Option<String>)>,
    badges: Vec<BadgeSurface>,
}

fn unsupported(what: &'static str) -> impl FnOnce(wayland_client::globals::BindError) -> Error {
    move |_| Error::Unsupported(what.into())
}

fn connection(e: impl std::fmt::Display) -> Error {
    Error::Connection(e.to_string())
}

pub(crate) fn show(
    state: &mut State,
    queue: &mut EventQueue<State>,
    qh: &QueueHandle<State>,
    globals: &GlobalList,
    labels: &[(String, usize)],
) -> Result<(), Error> {
    hide(state);
    if state.identify.globals.is_none() {
        state.identify.globals = Some(Globals {
            compositor: globals.bind(qh, 1..=4, ()).map_err(unsupported("wl_compositor"))?,
            shm: globals.bind(qh, 1..=1, ()).map_err(unsupported("wl_shm"))?,
            layer_shell: globals.bind(qh, 1..=4, ()).map_err(unsupported("zwlr_layer_shell_v1"))?,
        });
    }

    // Bind every output to learn its connector name (wl_output v4 `name`).
    let outputs: Vec<WlOutput> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == WlOutput::interface().name)
            .map(|g| globals.registry().bind(g.name, g.version.min(4), qh, ()))
            .collect()
    });
    state.identify.outputs = outputs.into_iter().map(|o| (o, None)).collect();
    queue.roundtrip(state).map_err(connection)?;

    let Identify { globals: Some(g), outputs, badges } = &mut state.identify else {
        return Ok(());
    };
    for (output, name) in outputs.iter() {
        let Some(&(_, number)) = name.as_ref().and_then(|n| labels.iter().find(|(l, _)| l == n)) else {
            continue;
        };
        let size = Badge::new(number);
        let surface = g.compositor.create_surface(qh, ());
        // Empty input region: clicks pass through the badge.
        let region = g.compositor.create_region(qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();

        let layer = g.layer_shell.get_layer_surface(
            &surface,
            Some(output),
            zwlr_layer_shell_v1::Layer::Overlay,
            "rsds-identify".into(),
            qh,
            (),
        );
        layer.set_size(size.width as u32, size.height as u32);
        layer.set_anchor(zwlr_layer_surface_v1::Anchor::Bottom | zwlr_layer_surface_v1::Anchor::Left);
        layer.set_margin(0, 0, badge::MARGIN, badge::MARGIN);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        surface.commit();
        badges.push(BadgeSurface { number, surface, layer, buffer: None });
    }
    queue.flush().map_err(connection)
}

pub(crate) fn hide(state: &mut State) {
    for b in state.identify.badges.drain(..) {
        b.destroy();
    }
    for (output, _) in state.identify.outputs.drain(..) {
        if output.version() >= 3 {
            output.release();
        }
    }
}

/// Draw a badge into a new shm buffer.
fn create_buffer(shm: &WlShm, badge: &Badge, qh: &QueueHandle<State>) -> std::io::Result<(WlShmPool, WlBuffer)> {
    let bytes: Vec<u8> = badge.pixels().iter().flat_map(|p| p.to_ne_bytes()).collect();
    let fd = rustix::fs::memfd_create("rsds-identify", rustix::fs::MemfdFlags::CLOEXEC)?;
    let mut file = std::fs::File::from(fd);
    file.write_all(&bytes)?;
    let pool = shm.create_pool(file.as_fd(), bytes.len() as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        badge.width,
        badge.height,
        badge.width * 4,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    Ok((pool, buffer))
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let id = &mut state.identify;
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                proxy.ack_configure(serial);
                let Some(b) = id.badges.iter_mut().find(|b| &b.layer == proxy) else { return };
                if b.buffer.is_none()
                    && let Some(g) = &id.globals
                {
                    let badge = Badge::new(b.number);
                    if let Ok((pool, buffer)) = create_buffer(&g.shm, &badge, qh) {
                        b.surface.attach(Some(&buffer), 0, 0);
                        b.surface.damage_buffer(0, 0, badge.width, badge.height);
                        b.buffer = Some((pool, buffer));
                    }
                }
                b.surface.commit();
            }
            zwlr_layer_surface_v1::Event::Closed => {
                if let Some(i) = id.badges.iter().position(|b| &b.layer == proxy) {
                    id.badges.swap_remove(i).destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(o) = state.identify.outputs.iter_mut().find(|(o, _)| o == proxy)
        {
            o.1 = Some(name);
        }
    }
}

/// Objects whose events we don't need.
macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}

ignore_events!(WlCompositor, WlShm, WlShmPool, WlBuffer, WlSurface, WlRegion, ZwlrLayerShellV1);
