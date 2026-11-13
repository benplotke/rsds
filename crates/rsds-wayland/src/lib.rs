//! Wayland backend for wlroots compositors, using wlr-output-management.

mod gamma;
mod identify;
mod output_management;

pub use output_management::WaylandBackend;
