# Keyboard mouse

Run `kbmouse.ahk` with [AutoHotkey v2](https://www.autohotkey.com/) on Windows.
Press **Caps Lock** to toggle mouse mode. Caps Lock's normal function is disabled while the script runs.

A click-through badge appears in the bottom-left corner of the primary screen, above the taskbar, while mouse mode is on. It shows the speed level and whether drag is held. The badge hides while the cursor is over its area and reappears when the cursor moves away. Turning mouse mode off hides it. Movement polling stops when all movement keys are released, and quitting releases any active drag.

## Controls

These bindings apply in mouse mode; Caps Lock and Ctrl+Alt+Q work in either mode.

| Key | Action |
| --- | --- |
| I / J / K / L | Move up / left / down / right |
| Space + movement | Precision movement (20% speed) |
| 1 / 2 / 3 / 4 | Jump near top-left / top-right / bottom-left / bottom-right corners of the cursor's current monitor (top number row) |
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
Speed levels are **800, 1600, 2400 (default), 3000, 4000 px/s** and reset on restart.
Holding a scroll key scrolls smoothly and continuously (tune `scrollSpeed`); a tap scrolls one notch. Space slows scrolling too. Ctrl + U / O sends whole notches, so it zooms like Ctrl + wheel.

Space slows movement only while held. Keys 1–4 jump once per press on whichever monitor currently contains the cursor, landing 10% in from the edges of its usable area, excluding the taskbar. Customize the four `[x, y]` fractions in `jumpPositions` near the top of the script (`0.0` is left/top, `0.5` center, `1.0` right/bottom). These number keys type normally outside mouse mode.

## Troubleshooting

- Keys type letters: press Caps Lock to enable mouse mode.
- Need normal typing: press Caps Lock to disable mouse mode.
- Script won't start: check that AutoHotkey **v2** is installed.
- An app ignores movement: try a normal desktop app; elevated apps and games may handle simulated input differently.
- Change bindings: edit `kbmouse.ahk` and reload it. Movement keys are also read in `MovePointer()`.
