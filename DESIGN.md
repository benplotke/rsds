# Design decisions

Decisions made during development that aren't in the README.

## Scope
- Wayland: wlroots compositors only (`wlr-output-management`, `wlr-gamma-control`, `wlr-layer-shell`).
- X11: RandR 1.3+ (CRTC transforms, primary output), over x11rb's pure-Rust connection
  (no libxcb).
- Talk to the display server protocols directly (`wayland-client`, `x11rb`) rather than
  shelling out to `wlr-randr`/`xrandr`: the daemon needs a live connection anyway for
  hotplug events and (on Wayland) to hold the Nightlight gamma ramp.
- Wayland apply: if the compositor rejects a configuration, it is retried one output at a
  time (turn-offs first, rejected steps retried after the others), restoring the original
  if it can't be reached. Labwc checks each output of a request against the current state
  of the others, so freeing display bandwidth on one monitor and using it on another only
  works in steps (e.g. two 4K monitors on one MST dock).
- Two binaries per package: `rsds` (the UI) and `rsds-daemon`, with the same names in the
  Wayland and X11 packages, which conflict with each other. A single binary with the
  daemon as a subcommand cost the daemon memory: the kernel reads ahead around the pages
  it uses, so most of the UI code was loaded too (measured PSS 1.24 MB with one binary,
  0.92 MB with its own).
  - Both packages' crates (`rsds-w`, `rsds-x`) are in one workspace, so building both at
    once (`cargo build --workspace`) clobbers `target/*/rsds`; build one with `-p`.
- The UI is launched from a terminal (no `.desktop` file).

## Features
- "Primary" is hidden on Wayland (wlroots has no such concept).
- X11 offers scale via `xrandr --scale`-style transforms, with a note that it isn't true scaling.
  - Scale s is the CRTC transform diag(1/s, 1/s, 1) (bilinear filter, nearest at 1), its
    16.16 factor rounded down so a monitor at 125% covers exactly 2048 of 2560 px. Read
    back as 1/factor rounded to 1/256.
- Primary (X11): one monitor or none; Yes on one sets No on the others; turning a monitor off
  makes it not primary. Saved in the profile as `"primary": true`. A profile without a
  primary monitor (e.g. saved on Wayland) leaves the primary as it is.
- X11 apply: with the server grabbed, CRTCs that go off, change output or don't fit the new
  screen size are turned off, the screen is resized to the layout's bounding box (keeping
  the DPI it had at startup), then each changed CRTC is set; an output that stays on keeps
  its CRTC. The layout is shifted so nothing is at a negative position. If a step is
  refused, the previous configuration is put back. `test` can only check what is known
  beforehand (modes, free CRTCs, maximum screen size).
- X11 keeps a CRTC going when its monitor is unplugged; the backend turns such CRTCs off
  (shrinking the screen) whenever it sees one.
- X11 monitor identity: make, model and serial are read from EDID as libdisplay-info names
  them (make from hwdata's `pnp.ids`, else the PNP ID; model `0x%04X` without a name;
  serial `0x%08X` without a serial string), so profiles carry over between X11 and Wayland.
- X11 modes that differ only in timings or sync flags (same size and refresh, to the mHz)
  are listed once, the first (preferred) kept.
- Nightlight: held by the daemon; fixed-time schedule only; adjustable colour temperature.
  - Settings in `~/.config/rsds/nightlight.json` (`enabled`, `temperature` in K, `schedule`,
    `start`/`end` as "HH:MM"). The UI writes it on every change; the daemon watches the
    directory with inotify and applies changes immediately.
  - `enabled` is whether the tint is on now, and the UI's On/Off shows and sets it. With
    `schedule` on, the daemon switches it on at `start` and off at `end` (equal times: no
    schedule), saving that to the file so the UI (which watches the file) shows it; in
    between, On/Off can be switched either way. A scheduled time that passed while the
    daemon wasn't running or the computer slept takes effect when it next looks, unless
    the file was written after it (the file's mtime is when the daemon starts from).
  - The daemon arms a CLOCK_REALTIME timerfd for the next start/end, so changes happen on
    time after suspend and clock changes (CANCEL_ON_SET). Local time via libc (DST-aware).
  - Wayland: wlr-gamma-control on every output, re-applied to outputs connected later.
    Colour from a black-body approximation (Tanner Helland), 1900–6500 K.
  - X11: RandR CRTC gamma on every active CRTC, re-applied when the active CRTCs or their
    modes change. Unlike on Wayland, the tint stays if the daemon exits.
  - The daemon holds a flock on `$XDG_RUNTIME_DIR/rsds-daemon.lock`; a second daemon
    refuses to start, and the UI uses it to warn when the daemon isn't running.
- UI sections: a left-most pane switches between Displays (sections · displays · settings
  · options), Nightlight (sections · settings · options) and Lid Close (sections · lid close
  · info, which explains only the chosen option).
- Footer: buttons apply · reset · identify · quit, whose highlighted first letter is the
  key; there are no separate key hints (arrow keys are left to be guessed). Enter applies.
  The confirmation dialog's buttons are keep · revert the same way (Enter/y and Esc/n
  also work, unadvertised).
- Identify: drawn by the UI process. Wayland: layer-shell + wl_shm buffer, digits drawn
  as simple blocky shapes (no font library). X11: one override-redirect window per monitor.
- Brightness: logind `SetBrightness` over D-Bus (no root, no udev rules), via the `dbus`
  crate (libdbus-1). Current level read from sysfs. Shown for internal panels (connector
  `eDP*`/`LVDS*`/`DSI*`) when a backlight device exists, preferring firmware > platform > raw.
  Applied immediately, outside Apply/Revert. 0% maps to the lowest non-zero level so the
  panel never switches off.
- Lid close: unless the setting is System default, the daemon holds a logind
  `handle-lid-switch` inhibitor and decides itself what closing the lid does, from the
  Lid Close setting (`~/.config/rsds/lid-close.json`, `lid_close`: `suspend`,
  `suspend_unless_docked` (default), `never` or `system_default`; watched like
  Nightlight's file):
  - Always suspend: the daemon calls logind's `Suspend`, docked or not (logind's default
    `HandleLidSwitchDocked=ignore` would otherwise keep a docked laptop awake).
  - Suspend unless docked: suspend without an external monitor; with one, turn the panel off.
  - Never suspend: with an external monitor, as above; without one, the panel's display
    is switched off (wlr-output-power-management) but it stays in the layout. X11 can only
    do this with DPMS, which switches off every display (only the panel is connected
    then) and switches back on at any input. DPMS is enabled for this if it was off, and
    disabled again when the lid opens.
  - System default: no inhibitor, so logind (`HandleLidSwitch*` in logind.conf) decides
    whether to suspend; the daemon never suspends. With an external monitor, the panel is
    still turned off as above. Without one, the daemon does nothing. Choosing it hands the
    lid back to logind at once (the inhibitor is dropped); choosing another option takes
    it over again.
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
- Dragging moves a monitor in whole map cells. On release it stays where it was drawn,
  moved only as far as needed to share an edge with another and overlap none. An edge
  drawn in line with the matching edge of a monitor it touches (top or bottom beside it,
  left or right above or below it) is moved to be exactly in line; where both its edges
  are drawn in line but can't both be, the top or left one wins. The map then keeps its
  scale (moving by whole cells if needed to show everything) until something else
  changes, so what was drawn is what you get.
- Monitors coming or going (including the panel when the lid takes it out of use) while
  the UI is open: unapplied edits are dropped and the UI shows the live state. During the
  "Keep these display settings?" countdown, the UI records the configuration to revert to
  in `$XDG_RUNTIME_DIR/rsds-unconfirmed.json` (with the countdown's end, so a stale file
  is ignored); the daemon takes it when the monitor set changes and handles the change
  starting from that configuration, as if the countdown had run out first. Without the
  daemon, the UI reverts by itself.
- Duplicate: wlroots has no mirroring, so a duplicate is an output at its source's position
  (labwc draws overlapping outputs with the same content; tested). The duplicate keeps its
  own resolution and gets the largest scale (in the protocol's 1/256 steps) at which it
  covers the source; with a different aspect ratio it shows a strip beyond the source along
  one edge. Scales below 1 are fine on labwc (tested 0.5).
  - Considered and dropped: centring a group's monitors with equal areas, so the shared
    area is their intersection with a border on each. The border isn't black: the
    compositor treats it as desktop, so the mouse and windows can go into it, and the
    output protocol can't crop an output. So the duplicates cover the source instead.
  - Groups are symmetric to the user: a source and its duplicates form a group, and every
    member's Duplicate setting shows the group ("1 | 3"). Internally one member (the one
    the others joined) is the source: its scale is the one that can be set, and the
    others' scales are greyed out and follow it.
  - The group takes part in the arrangement as one box, the source's; dragging any of it
    moves the group.
  - Choices for a monitor not in a group: Off, each other monitor that is on and alone,
    and each group as one option (choosing it joins the group). For a monitor in a group:
    Off and its group; to move to another group, leave first.
  - Off (or turning the monitor off) takes just that monitor out. If it was the source,
    the group stays where it is and the next member becomes the source, with the nearest
    regular scale; a group left with one monitor is no more. The monitor that left goes
    back to its remembered place (below) if it has one, else right of the others.
  - Dropping a monitor on its own onto another monitor's box duplicates it (joins its
    group): its centre must land in the middle half, across and down, of that monitor.
    A group can't be dropped onto another; it is placed like any drag.
  - The compositor doesn't report mirroring, so it is read from the layout: outputs at the
    same position where one covers the others (within 3 px of rounding). When they are the
    same size, which is the source comes from the UI's own edits, else the saved profile
    (`duplicates`: the source's identity), else the first.
  - The daemon keeps duplicates on their sources when it closes gaps.
- Remembered places: when a monitor leaves the arrangement (turned off, or made a
  duplicate), the layout from just before is remembered. When it comes back (turned on,
  or Duplicate Off), it and the others go back to their places, if the other monitors are
  still where they were right after it left (same position, size and duplicating);
  otherwise it goes to the right of the others, vertically centred on the rightmost.
  Only position, scale and who duplicates whom are restored; other settings stay as they
  are now.
  - Places are kept through Apply and saved with the profile on Keep (`restores`: by
    monitor identity, `before` gives the position, scale and source of each monitor that
    was on, `after` the position, size and source of each other monitor that was on right
    after), so they
    survive unplugging, replugging and restarting the UI. A place that no longer holds at
    Apply is dropped; on Keep the "after" layout is re-read from the live state to absorb
    the compositor's rounding.
  - Reset and Revert bring back the places of the kept configuration; a change of
    monitor set loads the new set's places from its profile.
  - With the lid closed, places are saved for the externals only.
- Monitor identity in profiles: EDID make|model|serial; falls back to connector name when
  there is no serial.
- `profiles.json` holds only what is used: a profile is its monitors (by identity; the set
  is the identities); one that is off is just `"enabled": false`; one that is on has its
  mode (without whether it is the preferred one), position, transform, scale and, for a
  duplicate, `duplicates`. Connector names aren't saved, as they can change.

## Daemon (`rsds-daemon`)
- Reacts only when the *set* of connected monitors changes (by identity), and once at startup.
  Ordinary configuration changes (from the UI, wlr-randr, …) are ignored.
- A profile is saved only when the user presses Keep in the UI. The daemon never saves
  profiles (it only writes Nightlight's `enabled` when the schedule switches it).
- Saved profile for the current set → apply it (also at startup).
- No profile:
  - new monitors are turned on at their preferred mode and placed right of the rightmost
    existing monitor, vertically centred on it; existing monitors are untouched;
  - when monitors were removed, the remaining ones slide together to close gaps;
  - if every monitor would be off, all are turned on at their preferred mode;
  - at startup, nothing is changed.

## Crates
- `rsds-core`: model, layout geometry (snapping, compaction, placing to the right),
  profiles and settings files, Identify badge, `Backend` trait.
- `rsds-wayland`: wlroots backend.
- `rsds-tui`: ratatui UI, generic over `Backend`.
- `rsds-daemon`: hotplug/profile service, generic over `Backend`.
- `rsds-logind`: logind/sysfs integration (backlight, lid state and inhibitor).
- `rsds-x11`: X11 (RandR) backend.
- `rsds-w`: the Wayland binaries, `rsds` and `rsds-daemon`.
- `rsds-x`: the X11 binaries, `rsds` and `rsds-daemon`.
