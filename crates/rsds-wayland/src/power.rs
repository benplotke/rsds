//! Turning an output's display off and on without changing the layout,
//! via wlr-output-power-management. The mode stays after we disconnect.

use rsds_core::Error;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    globals::GlobalList,
    protocol::wl_output::{self, WlOutput},
};
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1,
    zwlr_output_power_v1::{self, ZwlrOutputPowerV1},
};

use crate::output_management::State;

/// User data for the wl_outputs bound here, to tell them from others.
pub(crate) struct PowerOutput;

#[derive(Default)]
pub(crate) struct Power {
    manager: Option<ZwlrOutputPowerManagerV1>,
    /// wl_outputs bound during `set`, with their connector names.
    outputs: Vec<(WlOutput, Option<String>)>,
}

pub(crate) fn set(
    state: &mut State,
    queue: &mut EventQueue<State>,
    qh: &QueueHandle<State>,
    globals: &GlobalList,
    name: &str,
    on: bool,
) -> Result<(), Error> {
    let connection = |e: wayland_client::DispatchError| Error::Connection(e.to_string());
    if state.power.manager.is_none() {
        state.power.manager = Some(
            globals
                .bind(qh, 1..=1, ())
                .map_err(|_| Error::Unsupported("zwlr_output_power_manager_v1".into()))?,
        );
    }

    // Bind every output to learn its connector name (wl_output v4 `name`).
    state.power.outputs = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == WlOutput::interface().name)
            .map(|g| (globals.registry().bind(g.name, g.version.min(4), qh, PowerOutput), None))
            .collect()
    });
    queue.roundtrip(state).map_err(connection)?;

    let p = &mut state.power;
    let target = p.outputs.iter().find(|(_, n)| n.as_deref() == Some(name)).map(|(o, _)| o);
    let result = match (target, &p.manager) {
        (Some(output), Some(manager)) => {
            let control = manager.get_output_power(output, qh, ());
            control.set_mode(if on { zwlr_output_power_v1::Mode::On } else { zwlr_output_power_v1::Mode::Off });
            control.destroy();
            Ok(())
        }
        _ => Err(Error::NotFound(name.into())),
    };
    for (output, _) in p.outputs.drain(..) {
        if output.version() >= 3 {
            output.release();
        }
    }
    queue.flush().map_err(|e| Error::Connection(e.to_string()))?;
    result
}

impl Dispatch<WlOutput, PowerOutput> for State {
    fn event(
        state: &mut Self,
        proxy: &WlOutput,
        event: wl_output::Event,
        _: &PowerOutput,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(o) = state.power.outputs.iter_mut().find(|(o, _)| o == proxy)
        {
            o.1 = Some(name);
        }
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrOutputPowerManagerV1, _: <ZwlrOutputPowerManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrOutputPowerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrOutputPowerV1, _: zwlr_output_power_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
