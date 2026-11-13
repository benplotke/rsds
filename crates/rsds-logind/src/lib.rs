//! System integration through systemd-logind (D-Bus) and sysfs.

pub mod backlight;

pub use backlight::Backlight;
