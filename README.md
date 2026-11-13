# (rsds) Reasonably Sane Display Settings README

## Goal
This project aims to provide a high quality standalone manager for configuration of monitors. This includes sane defaults for newly plugged in monitors, automatic restoration of configuration when plugging into previously seen monitors, and an intuitive UI for configuring monitors. The feature set and experience are inspired by Windows 11's Display Settings. The UI is inspired by btop.

## Philosophy
Computers should do what we want, and no more. Programs should do what they do with as little resources as possible. What a program does should be done well.

## Issues With Existing Solutions
- lxrandr – does not support advanced features like arbitrary screen positioning
- xlayoutdisplay - does not support advanced features like arbitrary screen positioning
- arandr + autorandr – despite the name, you have to run CLI commands to save a configuration
- wdisplays + kanshi – This is very close to what I want. I don't think it support wl-roots out of the box though.
- way-displays – does not support advanced features like arbitrary screen positioning

## Design Choices
- Rust for the combination of performance and safety
- RataTUI for esthetics and small binary size
- An X11 version and a Wayland version to keep the binary size down for each
- JSON for saved configurations so the internal state of the program can be serialized in a readable format.

## UI Features
- Graphical view of the monitors which can be dragged with the mouse to rearrange
- Graphical view has automatic snapping
- All available monitors are visible in the UI, but disabled ones are grayed out
- Brightness slider for the built-in display, on laptops only
- Nightlight toggle with optional schedule
- Identify button which causes each screen to flash its number
- List of monitors with the settings for each
- Settings for each monitor are: on or off, primary, resolution, refresh rate, orientation and scale
- New layout reverts after 15 seconds if you don't select accept on the countdown

## Daemon Features
- Settings for previously configured monitor sets are saved as JSON to ~/.config/rsds/profiles.json
- When you plug in new monitor, the monitor is added to the right of your existing monitors. The existing screens continue to have their present size and windows.
- When monitors are added or removed, if the current set of monitors has settings, those settings are applied

## Other Features
- Closing the lid while external monitors are attached does not suspend the computer, rather reconfigures to just the external monitors

## Compatibility
The Wayland package was developed on Labwc and the X11 was developed on OpenBox. If you have compatibility issues, feel free to raise an issue.
