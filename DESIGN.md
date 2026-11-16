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
- Lid close: the daemon always holds a logind `handle-lid-switch` inhibitor and decides
  itself what closing the lid does, from the Suspend setting (`~/.config/rsds/suspend.json`,
  `lid_close`: `suspend`, `suspend_unless_docked` (default) or `never`; watched like
  Nightlight's file):
  - Always suspend: the daemon calls logind's `Suspend`, docked or not (logind's default
    `HandleLidSwitchDocked=ignore` would otherwise keep a docked laptop awake).
  - Suspend unless docked: suspend without an external monitor; with one, turn the panel off.
  - Never suspend: with an external monitor, as above; without one, the panel's display
    is switched off (wlr-output-power-management) but it stays in the layout.
  - Suspending happens only when the lid closes or the last external monitor is unplugged
    with the lid closed, never because the setting changed (so choosing Always suspend
    from the UI with the lid closed doesn't suspend on the spot).
  - "Docked" means any external monitor is connected.
  - If the daemon isn't running, logind's own lid settings apply.
  - Lid state from logind's `LidClosed` property (watched via `PropertiesChanged`).
  - With the lid closed and an external connected, the panel is left out of the monitor set
    (it is still connected, but turned off), so closing the lid is handled like unplugging
    the panel: the externals' profile is applied if there is one, otherwise gaps close.
    Opening it is handled like plugging the panel in: the full set's profile if there is
    one, otherwise the panel goes to the right. The pre-close layout isn't remembered.
  - Keep in the UI with the lid closed saves the profile for the externals only.
  - Window placement when the panel turns off is up to the compositor (labwc moves each
    window to the output nearest its centre, and back when the panel returns).
  - Only logind's lid handling can be inhibited; anything else that suspends on the lid
    (e.g. an acpid handler running `systemctl suspend`) must be turned off by the user.
- Dragging: on release, an edge within half a terminal cell (per axis) of another
  monitor's parallel edge snaps to it, so any alignment is reachable by dragging and
  moving one cell away from an aligned edge is never undone.
- Monitors coming or going (including the panel when the lid takes it out of use) while
  the UI is open: unapplied edits are dropped and the UI shows the live state. During the
  "Keep these display settings?" countdown, the UI records the configuration to revert to
  in `$XDG_RUNTIME_DIR/rsds-unconfirmed.json` (with the countdown's end, so a stale file
  is ignored); the daemon takes it when the monitor set changes and handles the change
  starting from that configuration, as if the countdown had run out first. Without the
  daemon, the UI reverts by itself.
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
- `rsds-logind`: logind/sysfs integration (backlight, lid state and inhibitor).
- `rsds-w`: Wayland binary.
- (to come) `rsds-x11`, `rsds-x`.
