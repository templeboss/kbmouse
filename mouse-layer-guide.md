# Keyboard mouse

Run `kbmouse.ahk` with [AutoHotkey v2](https://www.autohotkey.com/) on Windows.
Press **Caps Lock** to toggle mouse mode. Caps Lock's normal function is disabled while the script runs.

## Controls

These bindings apply in mouse mode; Caps Lock and Ctrl+Alt+Q work in either mode.

| Key | Action |
| --- | --- |
| I / J / K / L | Move up / left / down / right |
| Space + movement | Precision movement (20% speed) |
| ; | Left click (physical US semicolon key, `SC027`) |
| N | Right click |
| , | Middle click |
| M | Toggle left-button hold for dragging |
| U / O | Scroll up / down |
| P / . | Increase / decrease speed |
| Caps Lock | Toggle mouse mode; turning it off releases a drag |
| Ctrl+Alt+Q | Quit the script |

Hold movement keys for continuous movement; combine directions for diagonals.
Speed levels are **800, 1600, 2400 (default), 3000, 4000 px/s** and reset on restart.

## Troubleshooting

- Keys type letters: press Caps Lock to enable mouse mode.
- Need normal typing: press Caps Lock to disable mouse mode.
- Script won't start: check that AutoHotkey **v2** is installed.
- An app ignores movement: try a normal desktop app; elevated apps and games may handle simulated input differently.
- Change bindings: edit `kbmouse.ahk` and reload it. Movement keys are also read in `MovePointer()`.
