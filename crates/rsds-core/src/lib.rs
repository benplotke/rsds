//! Display-server independent model of monitors and their configuration.

pub mod badge;
pub mod config;
pub mod daemon_lock;
pub mod layout;
pub mod lid_close;
pub mod mirror;
pub mod nightlight;
pub mod output;
pub mod profile;
pub mod unconfirmed;

pub use output::{Mode, Output, OutputConfig, Transform};

/// A display server backend (Wayland or X11).
///
/// Backends are event driven: the caller polls `fd()` for readability and
/// calls `dispatch()` when it becomes readable.
pub trait Backend {
    /// File descriptor to poll for incoming display server events.
    fn fd(&self) -> std::os::fd::BorrowedFd<'_>;

    /// Process pending events without blocking.
    /// Returns true if the set of outputs or their state changed.
    fn dispatch(&mut self) -> Result<bool, Error>;

    /// Whether the display server has a primary monitor (X11 does,
    /// Wayland doesn't). Without one, `primary` is ignored.
    fn has_primary(&self) -> bool {
        false
    }

    /// Whether scale is real scaling (Wayland) rather than rendering at
    /// another size and resampling (X11).
    fn true_scaling(&self) -> bool {
        true
    }

    /// Current state of all connected outputs, enabled or not.
    fn outputs(&self) -> &[Output];

    /// Apply a full configuration. Outputs not mentioned are left unchanged.
    fn apply(&mut self, config: &[OutputConfig]) -> Result<(), Error>;

    /// Ask the display server whether `apply` would accept a configuration,
    /// without changing anything.
    fn test(&mut self, config: &[OutputConfig]) -> Result<(), Error>;

    /// Show a number badge on each named output until `end_identify`.
    /// `labels` pairs connector names with the number to show.
    fn identify(&mut self, labels: &[(String, usize)]) -> Result<(), Error>;

    /// Remove the badges shown by `identify`.
    fn end_identify(&mut self);

    /// Tint all outputs to a colour temperature (kelvin), or remove the
    /// tint with None. The backend keeps applying it to outputs that are
    /// connected later.
    fn set_color_temperature(&mut self, kelvin: Option<u32>) -> Result<(), Error>;

    /// Turn the named output's display off or back on. Unlike disabling
    /// it, this leaves the output where it is in the layout.
    fn set_power(&mut self, output: &str, on: bool) -> Result<(), Error>;
}

#[derive(Debug)]
pub enum Error {
    /// Lost or failed to establish the display server connection.
    Connection(String),
    /// The display server does not support a required protocol/extension.
    Unsupported(String),
    /// The display server rejected the configuration.
    Rejected,
    /// The configuration was outdated (outputs changed meanwhile); retry.
    Cancelled,
    /// A referenced output or mode does not exist.
    NotFound(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Connection(s) => write!(f, "display server connection: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Rejected => write!(f, "configuration rejected by display server"),
            Error::Cancelled => write!(f, "configuration cancelled; outputs changed"),
            Error::NotFound(s) => write!(f, "not found: {s}"),
        }
    }
}

impl std::error::Error for Error {}
