//! The Wayland binaries: `rsds` (the UI) and `rsds-daemon`.

use std::process::ExitCode;

/// Handle `-h`/`--help` and reject other arguments, connect to the
/// compositor and run `body`.
pub fn main(
    name: &str,
    about: &str,
    body: fn(rsds_wayland::WaylandBackend) -> std::io::Result<()>,
) -> ExitCode {
    let usage = format!("usage: {name}\n\n{about}");
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => {}
        ["-h" | "--help"] => {
            println!("{usage}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{usage}");
            return ExitCode::FAILURE;
        }
    }
    let result = rsds_wayland::WaylandBackend::connect()
        .map_err(std::io::Error::other)
        .and_then(body);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{name}: {e}");
            ExitCode::FAILURE
        }
    }
}
