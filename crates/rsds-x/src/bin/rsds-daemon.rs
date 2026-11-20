fn main() -> std::process::ExitCode {
    rsds_x::main(
        "rsds-daemon",
        "Background service that restores saved display layouts and runs Nightlight and Lid Close.",
        daemon::run,
    )
}
