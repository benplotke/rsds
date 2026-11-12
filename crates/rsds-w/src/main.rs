use std::process::ExitCode;

fn main() -> ExitCode {
    let backend = match rsds_wayland::WaylandBackend::connect() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("rsds: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = rsds_tui::run(backend) {
        eprintln!("rsds: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
