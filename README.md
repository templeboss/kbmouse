# kbmouse

Drive the mouse from the keyboard on Windows. Caps Lock toggles a mouse layer; while it is on, home-row keys move the cursor, click, scroll and jump around the screen.

Download [`kbmouse.exe`](https://github.com/templeboss/kbmouse/releases/latest/download/kbmouse.exe) (built from the newest commit) and run it. It asks for admin rights (UAC) on start so it also works over elevated windows.

## Build

```powershell
cargo build --release   # target\release\kbmouse.exe
.\rebuild.ps1           # stop the running exe, rebuild, restart it
```

## Mode

| Key | Action |
| --- | --- |
| Caps Lock | Toggle mouse mode (releases drag) |
| Shift + Caps Lock | Toggle the real Caps Lock |
| Ctrl + Alt + Q | Quit |

A badge in the bottom-left of the cursor's monitor shows the speed level while mouse mode is on. It turns amber while dragging and hides when the cursor is over it.

Click the tray icon for **Edit settings**, **Open settings folder**, **Reload settings** and **Quit**.

## Mouse mode keys

Defaults; every one can be rebound (see [Config](#config)). Keys are physical US-layout positions.

| Key | Action |
| --- | --- |
| I / J / K / L | Move up / left / down / right (combine for diagonals) |
| Space + move / scroll | Slow (20% movement, 25% scroll) |
| Shift + move / scroll | Fast (3×) |
| ; | Left click |
| N | Right click |
| , | Middle click |
| M | Toggle left-button hold (drag) |
| U / O | Scroll up / down |
| H / Y | Scroll left / right |
| Ctrl + U / O | One wheel notch per press (Ctrl + wheel zoom) |
| P / . | Speed level up / down |
| V / B | Browser back / forward (Ctrl + V / B pass through) |
| 7 / 9 | Home / End |
| 1 / 2 / 3 / 4 | Jump to top-left / top-right / bottom-left / bottom-right of the current monitor |
| ` | Jump to the center of the next monitor |
| G | Grid mode |
| F | Hint mode |

Holding a scroll key scrolls smoothly; a short tap scrolls in proportion to how long it was held. Unbound keys type normally. The speed level is remembered between runs.

### Grid mode

**G** covers the cursor's monitor with a 3×3 grid labelled `Q W E / A S D / Z X C` (`grid_rows = 4` adds `1 2 3` on top). Each key narrows to that cell and moves the cursor to its center; it closes on its own after the 4th press.

- **Shift** + cell key: left-click there and close
- `;` / `N` / `,`: close and click
- **Backspace**: step back
- **G**: start over with the full grid
- **Enter / Esc** or any other key: close

### Hint mode

**F** scans the focused window with UI Automation and labels its clickable controls (buttons, tabs, links, checkboxes, sliders, text fields) with one or two letters. Type a label to move there and click.

- **Shift** + last letter: move only, without clicking (e.g. to nudge along a seek bar)
- **Backspace**: erase a letter
- **Enter / Esc / F / Caps Lock**: close

Labels follow names where they can: **Q** close, **W** maximize/restore, **E** minimize for the window, otherwise the control's first letter. In large windows (e.g. browser pages) labels can take a moment to appear. Apps with no UI Automation tree (many games, custom-drawn UIs) get no labels; use grid mode there.

## Config

`%APPDATA%\kbmouse\kbmouse.ini` is created with defaults and comments on first run; open it from the tray icon. Use **Reload settings** in the tray menu to apply changes. Invalid lines fall back to their defaults and are listed in a warning.

| Setting | Default | Meaning |
| --- | --- | --- |
| `speeds` | `1800, 2400, 3000, 4000, 5200` | Speed levels in px/s (1–9 values) |
| `scroll_speed` | `1200` | Wheel units/s (120 = one notch) |
| `shift_boost` | `3.0` | Shift multiplier |
| `slow_move` / `slow_scroll` | `0.20` / `0.25` | Space multipliers |
| `grid_rows` | `3` | Grid rows, 3 or 4 |
| `disabled_apps` | `League of Legends.exe` | Exe names where kbmouse is fully off |
| `jump1`–`jump4` | 10% in from each corner | Corner jump targets as `x, y` fractions of the work area |
| `up`, `click_left`, … | see table above | Key bindings |

While a `disabled_apps` program is focused, every key passes through, Caps Lock works normally and mouse mode turns off.

Caps Lock, Shift, Ctrl and Ctrl + Alt + Q cannot be rebound.

## License

[PolyForm Noncommercial 1.0.0](LICENSE.md). Free for personal and other noncommercial use; any commercial use, including selling it or bundling it into a paid product, is not allowed. Copies and modified versions must keep the `Required Notice` line crediting Tymon Maciejak.
