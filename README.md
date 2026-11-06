# Keyboard mouse

Run `kbmouse.ahk` with [AutoHotkey v2](https://www.autohotkey.com/) on Windows.
Press **Caps Lock** to toggle mouse mode. Caps Lock's normal function is disabled while the script runs.

A click-through badge appears in the bottom-left corner of the monitor the cursor is on (it follows the cursor across monitors), above the taskbar, while mouse mode is on. It shows a mouse icon and the speed level (1–5); the icon turns amber while drag is held. The badge hides while the cursor is over its area and reappears when the cursor moves away. Turning mouse mode off hides it. Movement polling stops when all movement keys are released, and quitting releases any active drag.

## Controls

These bindings apply in mouse mode; Caps Lock and Ctrl+Alt+Q work in either mode.

| Key | Action |
| --- | --- |
| I / J / K / L | Move up / left / down / right |
| Space + movement | Precision movement (20% speed) |
| (startup) | Script relaunches itself elevated (UAC prompt on each start) |
| Shift + movement / scroll | Speed boost (1.5× movement, 2× scroll) |
| 1 / 2 / 3 / 4 | Jump near top-left / top-right / bottom-left / bottom-right corners of the cursor's current monitor (top number row) |
| ` | Jump to the center of the next monitor (cycles) |
| ; | Left click (physical US semicolon key, `SC027`) |
| N | Right click |
| , | Middle click |
| M | Toggle left-button hold for dragging |
| U / O | Scroll up / down |
| H / Y | Scroll left / right |
| B / V | Browser forward / back |
| P / . | Increase / decrease speed |
| Caps Lock | Toggle mouse mode; turning it off releases a drag |
| Ctrl+Alt+Q | Quit the script |

Hold movement keys for continuous movement; combine directions for diagonals.
Speed levels are **1800, 2400, 3000 (default), 4000, 5200 px/s** and reset on restart.
Holding a scroll key scrolls smoothly and continuously (tune `scrollSpeed`); a tap scrolls in proportion to how long the key is held. Space slows scrolling too. Ctrl + U / O sends whole notches, so it zooms like Ctrl + wheel.

Space slows movement only while held. Keys 1–4 jump once per press on whichever monitor currently contains the cursor, landing 10% in from the edges of its usable area, excluding the taskbar. Customize the four `[x, y]` fractions in `jumpPositions` near the top of the script (`0.0` is left/top, `0.5` center, `1.0` right/bottom). `` ` `` moves the cursor to the center of the next monitor, cycling through all of them. These number keys and `` ` `` type normally outside mouse mode.

## Rust version: grid mode

**G** opens a 3×3 grid over the cursor's monitor, labelled like the keys (`Q W E / A S D / Z X C`; `grid_rows = 4` adds `1 2 3` on top). Each key narrows to that cell and moves the cursor to its center; **Shift** + key also left-clicks there and closes the grid. After the 4th press the grid closes on its own. **Backspace** steps back; **Enter / Esc** close it, **G** restarts with the full grid (Caps closes it and leaves mouse mode); a **click key** (`;` `N` `,`) closes it and clicks. Any other key closes it (swallowed, like G).

## Rust version: config and state

The Rust version (`kbmouse-rs`) reads `%APPDATA%\kbmouse\kbmouse.ini`, created with defaults on first run. It sets speed levels, scroll speed, Shift boost, slow multipliers, corner jump positions and every mouse-mode key binding (keys are physical US positions, e.g. `click_left = semicolon`). Restart to apply changes. Bad values fall back to defaults and a warning box lists them at startup. Caps Lock, Shift, Ctrl and Ctrl+Alt+Q are fixed.

The current speed level is saved to `%APPDATA%\kbmouse\state.ini` and restored on start.

## Troubleshooting

- Keys type letters: press Caps Lock to enable mouse mode.
- Need normal typing: press Caps Lock to disable mouse mode.
- Script won't start: check that AutoHotkey **v2** is installed.
- An app ignores movement: try a normal desktop app; elevated apps and games may handle simulated input differently.
- Change bindings: edit `kbmouse.ahk` and reload it. Movement keys are also read in `MovePointer()`.
