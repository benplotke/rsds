use std::process::ExitCode;

const USAGE: &str = "usage: rsds-w [daemon]

  (no arguments)  open the display settings UI
  daemon          run the background service that restores saved layouts";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let daemon = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => false,
        ["daemon"] => true,
        ["-h" | "--help"] => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let backend = match rsds_wayland::WaylandBackend::connect() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("rsds: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = if daemon { rsds_daemon::run(backend) } else { rsds_tui::run(backend) };
    if let Err(e) = result {
        eprintln!("rsds: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
