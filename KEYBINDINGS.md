# Key bindings

The UI has an arrangement view at the top and three panes along the bottom:
**displays** (pick a monitor), **settings** (pick a setting), and
**options** (pick a value for that setting). Changes are pending until applied.

## Navigating the panes

| Key | Displays pane | Settings pane | Options pane |
|---|---|---|---|
| `↑` / `k` | previous monitor | previous setting | previous option |
| `↓` / `j` | next monitor | next setting | next option |
| `→` / `l` / `Enter` | go to settings | go to options | — |
| `←` / `h` | — | go to displays | go to settings |
| `Tab` | go to settings | go to options | go to displays |
| `Shift`+`Tab` | go to options | go to displays | go to settings |

Moving through the options changes the pending value immediately; nothing
reaches the display server until you apply.

**Brightness** (built-in panel only) is the exception: its options pane is a
slider, `↑` makes the panel brighter and `↓` dimmer in 5% steps, and the change
takes effect immediately. It is not part of Apply, Reset or the revert countdown.

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
| Click a setting | Select that setting |
| Click an option | Choose that option |
| Click or drag on the brightness slider | Set brightness to that point |
| Scroll wheel over a pane | Same as `↑` / `↓` in that pane |
| Click **Apply** / **Reset** / **Identify** | Same as `a` / `r` / `i` |
| Click **Keep** / **Revert** in the dialog | Keep or revert |
