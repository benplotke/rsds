# Design decisions

Decisions made during development that aren't in the README.

## Scope
- Wayland: wlroots compositors only (`wlr-output-management`, `wlr-gamma-control`, `wlr-layer-shell`).
- Talk to the display server protocols directly (`wayland-client`, `x11rb`) rather than
  shelling out to `wlr-randr`/`xrandr`: the daemon needs a live connection anyway for
  hotplug events and (on Wayland) to hold the Nightlight gamma ramp.
- Wayland apply: if the compositor rejects a configuration, it is retried one output at a
  time (turn-offs first, rejected steps retried after the others), restoring the original
  if it can't be reached. Labwc checks each output of a request against the current state
  of the others, so freeing display bandwidth on one monitor and using it on another only
  works in steps (e.g. two 4K monitors on one MST dock).
- One binary per package; the daemon is a subcommand. Unused UI code is never paged in,
  so a single binary costs the daemon essentially no memory.
- The UI is launched from a terminal (no `.desktop` file).

## Features
- "Primary" is hidden on Wayland (wlroots has no such concept).
- X11 offers scale via `xrandr --scale`-style transforms, with a note that it isn't true scaling.
- Nightlight: held by the daemon; fixed-time schedule only; adjustable colour temperature.
  - Settings in `~/.config/rsds/nightlight.json` (`enabled`, `temperature` in K, `schedule`,
    `start`/`end` as "HH:MM"). The UI writes it on every change; the daemon watches the
    directory with inotify and applies changes immediately.
  - `enabled` is the master switch; with `schedule` on, the tint is only active between
    `start` and `end` (may wrap past midnight; equal times = all day).
  - The daemon arms a CLOCK_REALTIME timerfd for the next start/end, so changes happen on
    time after suspend and clock changes (CANCEL_ON_SET). Local time via libc (DST-aware).
  - Wayland: wlr-gamma-control on every output, re-applied to outputs connected later.
    Colour from a black-body approximation (Tanner Helland), 1900–6500 K.
  - The daemon holds a flock on `$XDG_RUNTIME_DIR/rsds-daemon.lock`; a second daemon
    refuses to start, and the UI uses it to warn when the daemon isn't running.
- UI sections: a left-most pane switches between Displays (sections · displays · settings
  · options) and Nightlight (sections · settings · options).
- Identify: drawn by the UI process. Wayland: layer-shell + wl_shm buffer, digits drawn
  as simple blocky shapes (no font library). X11: one override-redirect window per monitor.
- Brightness: logind `SetBrightness` over D-Bus (no root, no udev rules), via the `dbus`
  crate (libdbus-1). Current level read from sysfs. Shown for internal panels (connector
  `eDP*`/`LVDS*`/`DSI*`) when a backlight device exists, preferring firmware > platform > raw.
  Applied immediately, outside Apply/Revert. 0% maps to the lowest non-zero level so the
  panel never switches off.
- Lid close: the daemon holds a logind `handle-lid-switch` inhibitor only while external
  monitors are connected, and turns off the internal panel when the lid closes.
- Monitor identity in profiles: EDID make|model|serial; falls back to connector name when
  there is no serial.

## Daemon (`rsds-w daemon`)
- Reacts only when the *set* of connected monitors changes (by identity), and once at startup.
  Ordinary configuration changes (from the UI, wlr-randr, …) are ignored.
- A profile is saved only when the user presses Keep in the UI. The daemon never saves.
- Saved profile for the current set → apply it (also at startup).
- No profile:
  - new monitors are turned on at their preferred mode and placed right of the rightmost
    existing monitor, top-aligned with it; existing monitors are untouched;
  - when monitors were removed, the remaining ones slide together to close gaps;
  - if every monitor would be off, all are turned on at their preferred mode;
  - at startup, nothing is changed.

## Crates
- `rsds-core`: model, layout geometry (snapping, compaction), profiles, Identify badge, `Backend` trait.
- `rsds-wayland`: wlroots backend.
- `rsds-tui`: ratatui UI, generic over `Backend`.
- `rsds-daemon`: hotplug/profile service, generic over `Backend`.
- `rsds-logind`: logind/sysfs integration (backlight; later the lid inhibitor).
- `rsds-w`: Wayland binary.
- (to come) `rsds-x11`, `rsds-x`.
