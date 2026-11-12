# Design decisions

Decisions made during development that aren't in the README.

## Scope
- Wayland: wlroots compositors only (`wlr-output-management`, `wlr-gamma-control`, `wlr-layer-shell`).
- Talk to the display server protocols directly (`wayland-client`, `x11rb`) rather than
  shelling out to `wlr-randr`/`xrandr`: the daemon needs a live connection anyway for
  hotplug events and (on Wayland) to hold the Night Light gamma ramp.
- One binary per package; the daemon is a subcommand. Unused UI code is never paged in,
  so a single binary costs the daemon essentially no memory.
- The UI is launched from a terminal (no `.desktop` file).

## Features
- "Primary" is hidden on Wayland (wlroots has no such concept).
- X11 offers scale via `xrandr --scale`-style transforms, with a note that it isn't true scaling.
- Night Light: held by the daemon; fixed-time schedule only; adjustable colour temperature.
- Identify: drawn by the UI process. Wayland: layer-shell + wl_shm buffer, digits drawn
  as simple blocky shapes (no font library). X11: one override-redirect window per monitor.
- Brightness: logind `SetBrightness` over D-Bus (no root, no udev rules). Shown for
  internal panels (connector `eDP*`/`LVDS*`/`DSI*`).
- Lid close: the daemon holds a logind `handle-lid-switch` inhibitor only while external
  monitors are connected, and turns off the internal panel when the lid closes.
- Monitor identity in profiles: EDID make|model|serial; falls back to connector name when
  there is no serial.

## Crates
- `rsds-core`: model, layout geometry (snapping), `Backend` trait.
- `rsds-wayland`: wlroots backend.
- `rsds-tui`: ratatui UI, generic over `Backend`.
- `rsds-w`: Wayland binary.
- (to come) `rsds-x11`, `rsds-x`.
