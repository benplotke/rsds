//! Development aid: print the outputs as seen by the backend.
use rsds_core::Backend;

fn main() {
    let backend = rsds_x11::X11Backend::connect().expect("connect");
    for o in backend.outputs() {
        println!(
            "{} [{}] primary={} enabled={} internal={} mode={:?} pos={:?} transform={:?} scale={} modes={}",
            o.name,
            o.identity(),
            o.primary,
            o.enabled,
            o.is_internal(),
            o.mode(),
            o.position,
            o.transform,
            o.scale,
            o.modes.len()
        );
    }
}
