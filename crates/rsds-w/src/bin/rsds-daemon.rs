fn main() -> std::process::ExitCode {
    rsds_w::main(
        "rsds-daemon",
        "Background service that restores saved display layouts and runs Nightlight and Lid Close.",
        daemon::run,
    )
}
