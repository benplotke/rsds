# Key bindings

The UI has an arrangement view at the top and panes along the bottom. The
left-most pane picks a section, **Displays**, **Nightlight** or **Lid Close**; the UI starts in
the Displays section with the monitor list focused.

- Displays section: **sections** · **displays** (pick a monitor) ·
  **settings** (pick a setting) · **options** (pick a value for that setting).
- Nightlight section: **sections** · **settings** · **options**.
- Lid Close section: **sections** · **lid close** (the choices).

## Navigating the panes

| Key | Sections pane | Displays pane | Settings pane | Options pane |
|---|---|---|---|---|
| `↑` / `k` | previous section | previous monitor | previous setting | previous option |
| `↓` / `j` | next section | next monitor | next setting | next option |
| `→` / `l` / `Enter` | pane to the right | pane to the right | go to options | — |
| `←` / `h` | — | go to sections | pane to the left | go to settings |
| `Tab` / `Shift`+`Tab` | cycle forwards / backwards through the section's panes | | | |

In the Displays section, moving through the options changes the pending value
immediately; nothing reaches the display server until you apply.

**Brightness** (built-in panel only) is the exception: its options pane is a
slider, `↑` makes the panel brighter and `↓` dimmer in 5% steps, and the change
takes effect immediately. It is not part of Apply, Reset or the revert countdown.

**Nightlight** and **Lid Close** settings also take effect immediately (they are
saved and applied by the daemon, `rsds-w daemon`, which must be running). Its temperature is a
slider: `↑` cooler (less tint), `↓` warmer, in 100 K steps from 1900 K to 6500 K.

The schedule's times use a picker with an hour column and a minute column:

| Key | Hour column | Minute column |
|---|---|---|
| `↑` / `k` | one hour earlier | five minutes earlier |
| `↓` / `j` | one hour later | five minutes later |
| `→` / `l` / `Enter` | go to the minute column | — |
| `←` / `h` | go to settings | go to the hour column |
| `0`–`9` | type the hour; after two digits typing continues in the minute column | type the minutes |

A typed value that is too large becomes the largest valid one (23 or 59).

## Actions

| Key | Action |
|---|---|
| `a` | Apply pending changes |
| `r` | Reset: discard pending changes |
| `i` | Identify: show each monitor's number on it for 3 seconds |
| `q` / `Ctrl`+`c` | Quit (reverts first if a confirmation is pending) |

## "Keep these display settings?" dialog

Shown after applying. Reverts automatically after 15 seconds.

| Key | Action |
|---|---|
| `Enter` / `y` / `k` | Keep (and save as the profile for the connected monitors) |
| `Esc` / `n` | Revert now |
| `q` / `Ctrl`+`c` | Revert and quit |

## Mouse

| Action | Effect |
|---|---|
| Drag a monitor in the arrangement | Move it; on release it snaps next to the others |
| Click a monitor or a row in the displays pane | Select that monitor |
| Click a section | Switch to that section |
| Click a setting | Select that setting |
| Click an option | Choose that option (in the time picker, that hour or minute) |
| Click or drag on a slider | Set brightness or temperature to that point |
| Scroll wheel over a pane | Same as `↑` / `↓` in that pane |
| Click **Apply** / **Reset** / **Identify** | Same as `a` / `r` / `i` |
| Click **Keep** / **Revert** in the dialog | Keep or revert |
