//! Nightlight on Wayland: gamma ramps via wlr-gamma-control.
//!
//! A gamma ramp only lasts while our gamma control object exists, so the
//! process that sets it (the daemon) must stay connected. Destroying the
//! control restores the original ramp.

use std::{
    io::{Seek, SeekFrom, Write},
    os::fd::AsFd,
};

use rsds_core::{Error, nightlight::gamma_ramp};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::GlobalList,
    protocol::{wl_output::WlOutput, wl_registry},
};
use wayland_protocols_wlr::gamma_control::v1::client::{
    zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
    zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
};

use crate::output_management::State;

struct GammaOutput {
    /// Registry name of the wl_output global.
    global: u32,
    output: WlOutput,
    control: ZwlrGammaControlV1,
    /// Ramp size, known after the compositor's `gamma_size` event.
    size: Option<u32>,
}

impl GammaOutput {
    fn destroy(self) {
        self.control.destroy();
        if self.output.version() >= 3 {
            self.output.release();
        }
    }
}

#[derive(Default)]
pub(crate) struct Gamma {
    manager: Option<ZwlrGammaControlManagerV1>,
    kelvin: Option<u32>,
    outputs: Vec<GammaOutput>,
}

pub(crate) fn set(
    state: &mut State,
    qh: &QueueHandle<State>,
    globals: &GlobalList,
    kelvin: Option<u32>,
) -> Result<(), Error> {
    let g = &mut state.gamma;
    let Some(kelvin) = kelvin else {
        g.kelvin = None;
        for o in g.outputs.drain(..) {
            o.destroy();
        }
        return Ok(());
    };
    if g.manager.is_none() {
        g.manager = Some(
            globals
                .bind(qh, 1..=1, ())
                .map_err(|_| Error::Unsupported("zwlr_gamma_control_manager_v1".into()))?,
        );
    }
    g.kelvin = Some(kelvin);
    if g.outputs.is_empty() {
        let outputs: Vec<(u32, u32)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|x| x.interface == WlOutput::interface().name)
                .map(|x| (x.name, x.version))
                .collect()
        });
        for (name, version) in outputs {
            add_output(g, globals.registry(), qh, name, version);
        }
    } else {
        for o in &g.outputs {
            apply(o, kelvin, qh);
        }
    }
    Ok(())
}

fn add_output(g: &mut Gamma, registry: &wl_registry::WlRegistry, qh: &QueueHandle<State>, name: u32, version: u32) {
    let Some(manager) = &g.manager else { return };
    let output: WlOutput = registry.bind(name, version.min(4), qh, name);
    let control = manager.get_gamma_control(&output, qh, ());
    g.outputs.push(GammaOutput { global: name, output, control, size: None });
}

fn apply(o: &GammaOutput, kelvin: u32, _qh: &QueueHandle<State>) {
    let Some(size) = o.size else { return };
    let bytes: Vec<u8> = gamma_ramp(size as usize, kelvin)
        .iter()
        .flat_map(|v| v.to_ne_bytes())
        .collect();
    let write = || -> std::io::Result<std::fs::File> {
        let fd = rustix::fs::memfd_create("rsds-gamma", rustix::fs::MemfdFlags::CLOEXEC)?;
        let mut file = std::fs::File::from(fd);
        file.write_all(&bytes)?;
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    };
    match write() {
        Ok(file) => o.control.set_gamma(file.as_fd()),
        Err(e) => eprintln!("rsds: can't create gamma ramp: {e}"),
    }
}

/// Registry events: follow outputs being connected and disconnected while
/// a tint is active.
pub(crate) fn registry_event(
    state: &mut State,
    registry: &wl_registry::WlRegistry,
    event: wl_registry::Event,
    qh: &QueueHandle<State>,
) {
    let g = &mut state.gamma;
    if g.kelvin.is_none() {
        return;
    }
    match event {
        wl_registry::Event::Global { name, interface, version } if interface == WlOutput::interface().name => {
            add_output(g, registry, qh, name, version);
        }
        wl_registry::Event::GlobalRemove { name } => {
            if let Some(i) = g.outputs.iter().position(|o| o.global == name) {
                g.outputs.swap_remove(i).destroy();
            }
        }
        _ => {}
    }
}

impl Dispatch<ZwlrGammaControlV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ZwlrGammaControlV1,
        event: zwlr_gamma_control_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let g = &mut state.gamma;
        let Some(i) = g.outputs.iter().position(|o| &o.control == proxy) else { return };
        match event {
            zwlr_gamma_control_v1::Event::GammaSize { size } => {
                g.outputs[i].size = Some(size);
                if let Some(k) = g.kelvin {
                    apply(&g.outputs[i], k, qh);
                }
            }
            zwlr_gamma_control_v1::Event::Failed => {
                // Another program (e.g. gammastep, wlsunset) holds the
                // gamma for this output, or it has no gamma support.
                eprintln!("rsds: gamma control failed on an output (is another nightlight program running?)");
                let o = g.outputs.swap_remove(i);
                if o.output.version() >= 3 {
                    o.output.release();
                }
                o.control.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrGammaControlManagerV1,
        _: <ZwlrGammaControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// wl_outputs bound for gamma; user data is the registry name.
impl Dispatch<WlOutput, u32> for State {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: <WlOutput as Proxy>::Event,
        _: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
