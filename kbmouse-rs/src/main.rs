#![windows_subsystem = "windows"]

//! Keyboard-driven mouse. Caps Lock toggles a mouse layer (see README.md).
//! A low-level keyboard hook swallows layer keys; a worker thread drives
//! cursor movement and smooth scrolling; a small layered window is the badge.

use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::thread::Thread;
use std::time::{Duration, Instant};

use windows::core::{w, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Worker tick while moving or scrolling; it sleeps until woken otherwise.
const TICK: Duration = Duration::from_millis(2);

// ---- virtual keys (fixed, not remappable) ----
const LSHIFT: usize = 0xA0;
const RSHIFT: usize = 0xA1;
const LCTRL: usize = 0xA2;
const RCTRL: usize = 0xA3;
const LALT: usize = 0xA4;
const RALT: usize = 0xA5;
const CAPS: usize = 0x14;
const KEY_Q: usize = 0x51;

// ---- config ----
#[derive(Clone, Copy, PartialEq)]
enum Action {
    Up, Down, Left, Right, Slow,
    ClickLeft, ClickRight, ClickMiddle, Drag,
    ScrollUp, ScrollDown, ScrollLeft, ScrollRight,
    SpeedUp, SpeedDown, Back, Forward, Home, End,
    Corner1, Corner2, Corner3, Corner4, NextMonitor, Grid, Hint,
}
use Action::*;

/// (config name, action, default key). Keys are physical positions (US layout).
const BINDINGS: [(&str, Action, &str); 26] = [
    ("up", Up, "I"), ("down", Down, "K"), ("left", Left, "J"), ("right", Right, "L"),
    ("slow", Slow, "space"),
    ("click_left", ClickLeft, "semicolon"), ("click_right", ClickRight, "N"),
    ("click_middle", ClickMiddle, "comma"), ("drag", Drag, "M"),
    ("scroll_up", ScrollUp, "U"), ("scroll_down", ScrollDown, "O"),
    ("scroll_left", ScrollLeft, "H"), ("scroll_right", ScrollRight, "Y"),
    ("speed_up", SpeedUp, "P"), ("speed_down", SpeedDown, "period"),
    ("back", Back, "V"), ("forward", Forward, "B"),
    ("home", Home, "7"), ("end", End, "9"),
    ("corner1", Corner1, "1"), ("corner2", Corner2, "2"),
    ("corner3", Corner3, "3"), ("corner4", Corner4, "4"),
    ("next_monitor", NextMonitor, "backtick"), ("grid", Grid, "G"), ("hint", Hint, "F"),
];

const DEFAULT_SPEEDS: &str = "1800, 2400, 3000, 4000, 5200";
const DEFAULT_DISABLED: &str = "League of Legends.exe";
const DEFAULT_JUMPS: [&str; 4] = ["0.10, 0.10", "0.90, 0.10", "0.10, 0.90", "0.90, 0.90"];

struct Config {
    speeds: Vec<f64>,
    scroll_speed: f64, // wheel units/s (120 = one notch)
    shift_boost: f64,
    slow_move: f64,
    slow_scroll: f64,
    grid_rows: usize,
    /// Lowercase exe names where kbmouse stays out of the way.
    disabled_apps: Vec<String>,
    jumps: [[f64; 2]; 4],
    /// Scan code bound to each action (indexed by `Action as usize`).
    key_of: [usize; BINDINGS.len()],
    /// Action bound to each scan code.
    action_at: [Option<Action>; 256],
}

static CFG: OnceLock<Config> = OnceLock::new();
fn cfg() -> &'static Config {
    CFG.get().expect("config loaded before hooks")
}

/// Physical key position -> set-1 scan code (US layout names).
fn key_sc(name: &str) -> Option<usize> {
    let n = name.trim().to_ascii_lowercase();
    if let [c] = n.as_bytes() {
        for (row, base) in [(&b"qwertyuiop"[..], 0x10), (b"asdfghjkl", 0x1E), (b"zxcvbnm", 0x2C)] {
            if let Some(i) = row.iter().position(|r| r == c) {
                return Some(base + i);
            }
        }
        match c {
            b'1'..=b'9' => return Some(0x02 + (c - b'1') as usize),
            b'0' => return Some(0x0B),
            _ => {}
        }
    }
    Some(match n.as_str() {
        "space" => 0x39,
        "semicolon" | ";" => 0x27,
        "comma" | "," => 0x33,
        "period" | "." => 0x34,
        "slash" | "/" => 0x35,
        "backtick" | "`" => 0x29,
        "minus" | "-" => 0x0C,
        "equals" | "=" => 0x0D,
        "lbracket" | "[" => 0x1A,
        "rbracket" | "]" => 0x1B,
        "quote" | "'" => 0x28,
        "backslash" | "\\" => 0x2B,
        _ => return None,
    })
}

fn parse_nums(v: &str) -> Option<Vec<f64>> {
    v.split(',').map(|p| p.trim().parse::<f64>().ok().filter(|x| x.is_finite())).collect()
}

fn appdata_dir() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("APPDATA")?).join("kbmouse"))
}

fn default_config_text() -> String {
    let mut t = String::from(
        "# kbmouse config. One `name = value` per line; # starts a comment.\n\
         # Delete or break a line to get its default (a warning shows at startup).\n\
         # Keys are physical positions (US layout): A-Z, 0-9, space, semicolon, comma,\n\
         # period, slash, backtick, minus, equals, lbracket, rbracket, quote, backslash.\n\
         # Caps Lock, Shift, Ctrl and Ctrl+Alt+Q are fixed.\n\n\
         # Speed levels in px/s (1-9 values); the badge shows the level.\n",
    );
    t += &format!("speeds = {DEFAULT_SPEEDS}\n");
    t += "scroll_speed = 1200   # wheel units/s, 120 = one notch\n";
    t += "shift_boost = 3.0     # Shift multiplies movement and scroll\n";
    t += "slow_move = 0.20      # slow key multiplier for movement\n";
    t += "slow_scroll = 0.25    # slow key multiplier for scrolling\n\n";
    t += "# Grid mode: 3 columns; 3 rows = QWE/ASD/ZXC, 4 rows adds 123 on top.\ngrid_rows = 3\n\n";
    t += "# Apps where kbmouse is fully off (comma-separated exe names, e.g. games).
";
    t += &format!("disabled_apps = {DEFAULT_DISABLED}

");
    t += "# Corner jump positions as x, y fractions of the work area (0 = left/top, 1 = right/bottom).\n";
    for (i, j) in DEFAULT_JUMPS.iter().enumerate() {
        t += &format!("jump{} = {j}\n", i + 1);
    }
    t += "\n# Key bindings.\n";
    for (name, _, key) in BINDINGS {
        t += &format!("{name} = {key}\n");
    }
    t
}

/// Reads `kbmouse.ini` (creating it with defaults if missing). Bad values fall
/// back to defaults and are collected in `warns`.
fn load_config(warns: &mut Vec<String>) -> Config {
    let dir = appdata_dir();
    let path = dir.as_ref().map(|d| d.join("kbmouse.ini"));
    let mut text = String::new();
    if let Some(p) = &path {
        match std::fs::read_to_string(p) {
            Ok(t) => text = t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::create_dir_all(dir.as_ref().unwrap());
                let _ = std::fs::write(p, default_config_text());
            }
            Err(e) => warns.push(format!("cannot read {}: {e}", p.display())),
        }
    }
    let mut map = std::collections::HashMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        match line.split_once('=') {
            Some((k, v)) => {
                map.insert(k.trim().to_ascii_lowercase(), (n + 1, v.trim().to_string()));
            }
            None => warns.push(format!("line {}: expected name = value", n + 1)),
        }
    }
    let known = |k: &str| {
        matches!(k, "speeds" | "scroll_speed" | "shift_boost" | "slow_move" | "slow_scroll" | "grid_rows" | "disabled_apps")
            || (k.len() == 5 && k.starts_with("jump") && matches!(k.as_bytes()[4], b'1'..=b'4'))
            || BINDINGS.iter().any(|b| b.0 == k)
    };
    for (k, (n, _)) in &map {
        if !known(k) {
            warns.push(format!("line {n}: unknown setting '{k}'"));
        }
    }

    let num = |name: &str, default: f64, warns: &mut Vec<String>| -> f64 {
        match map.get(name) {
            None => default,
            Some((n, v)) => match v.parse::<f64>() {
                Ok(x) if x.is_finite() && x > 0.0 => x,
                _ => {
                    warns.push(format!("line {n}: {name} must be a positive number"));
                    default
                }
            },
        }
    };
    let scroll_speed = num("scroll_speed", 1200.0, warns);
    let shift_boost = num("shift_boost", 3.0, warns);
    let slow_move = num("slow_move", 0.20, warns);
    let slow_scroll = num("slow_scroll", 0.25, warns);

    let mut grid_rows = 3;
    if let Some((n, v)) = map.get("grid_rows") {
        match v.parse::<usize>() {
            Ok(r @ (3 | 4)) => grid_rows = r,
            _ => warns.push(format!("line {n}: grid_rows must be 3 or 4")),
        }
    }

    let disabled_apps = map
        .get("disabled_apps")
        .map_or(DEFAULT_DISABLED, |(_, v)| v.as_str())
        .split(',')
        .map(|a| a.trim().to_lowercase())
        .filter(|a| !a.is_empty())
        .collect();

    let mut speeds = parse_nums(DEFAULT_SPEEDS).unwrap();
    if let Some((n, v)) = map.get("speeds") {
        match parse_nums(v) {
            Some(s) if (1..=9).contains(&s.len()) && s.iter().all(|x| *x > 0.0) => speeds = s,
            _ => warns.push(format!("line {n}: speeds needs 1-9 positive numbers")),
        }
    }

    let mut jumps = [[0.0; 2]; 4];
    for i in 0..4 {
        let d = parse_nums(DEFAULT_JUMPS[i]).unwrap();
        jumps[i] = [d[0], d[1]];
        if let Some((n, v)) = map.get(&format!("jump{}", i + 1)) {
            match parse_nums(v) {
                Some(p) if p.len() == 2 && p.iter().all(|x| (0.0..=1.0).contains(x)) => jumps[i] = [p[0], p[1]],
                _ => warns.push(format!("line {n}: jump{} needs two numbers between 0 and 1", i + 1)),
            }
        }
    }

    let mut key_of = [0usize; BINDINGS.len()];
    let mut action_at = [None; 256];
    for (name, action, default) in BINDINGS {
        let mut sc = key_sc(default).unwrap();
        if let Some((n, v)) = map.get(name) {
            match key_sc(v) {
                Some(s) => sc = s,
                None => warns.push(format!("line {n}: unknown key '{v}' for {name}, using {default}")),
            }
        }
        key_of[action as usize] = sc;
        if action_at[sc].is_some() {
            warns.push(format!("a key is bound twice; {name} is disabled"));
        } else {
            action_at[sc] = Some(action);
        }
    }
    Config { speeds, scroll_speed, shift_boost, slow_move, slow_scroll, grid_rows, disabled_apps, jumps, key_of, action_at }
}

// ---- persisted state (speed level) ----
fn state_path() -> Option<std::path::PathBuf> {
    Some(appdata_dir()?.join("state.ini"))
}

fn load_state() -> usize {
    let text = state_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let lvl = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "speed_level")
        .and_then(|(_, v)| v.trim().parse::<usize>().ok());
    lvl.unwrap_or(3).clamp(1, cfg().speeds.len())
}

fn save_state() {
    if let Some(p) = state_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, format!("speed_level = {}\n", SPEED_LEVEL.load(Relaxed)));
    }
}

fn warn_box(msg: &str) {
    let text: Vec<u16> = msg.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(None, PCWSTR(text.as_ptr()), w!("kbmouse config"), MB_OK | MB_ICONWARNING);
    }
}

const WM_REFRESH: u32 = WM_APP + 1;
const WM_MODE: u32 = WM_APP + 2;
const WM_SAVE_SOON: u32 = WM_APP + 3;
const WM_GRID: u32 = WM_APP + 4;
const WM_HINT: u32 = WM_APP + 5;
const TIMER_BADGE: usize = 1;
const TIMER_SAVE: usize = 2;

// ---- shared state ----
const fn flags() -> [AtomicBool; 256] {
    [const { AtomicBool::new(false) }; 256]
}
/// Physical (non-injected) key state, tracked for every key.
static HELD: [AtomicBool; 256] = flags();
/// Same, by scan code (non-extended keys only); mouse-layer keys use this.
static HELD_SC: [AtomicBool; 256] = flags();
/// Keys whose down event we swallowed, so the matching up is swallowed too.
static SWALLOWED: [AtomicBool; 256] = flags();
/// Keys pressed while hint mode was open: they stay inert until released, so a
/// label key (e.g. J = move left) can't start cursor movement after the pick.
static HINT_USED: [AtomicBool; 256] = flags();
/// Scroll keys armed in mouse mode: u, o, h, y.
static SCROLL: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];
/// True while the foreground app is in `disabled_apps`.
static DISABLED: AtomicBool = AtomicBool::new(false);
static MODE: AtomicBool = AtomicBool::new(false);
static DRAGGING: AtomicBool = AtomicBool::new(false);
static RESET: AtomicBool = AtomicBool::new(false);
/// True while smooth scrolling; physical Shift is hidden from apps then.
static SCROLLING: AtomicBool = AtomicBool::new(false);
static SHIFT_HIDDEN: AtomicBool = AtomicBool::new(false);
static SPEED_LEVEL: AtomicUsize = AtomicUsize::new(3);
static BADGE: AtomicIsize = AtomicIsize::new(0);
static WORKER: OnceLock<Thread> = OnceLock::new();
static BADGE_MONITOR: AtomicIsize = AtomicIsize::new(0);
static BADGE_VISIBLE: AtomicBool = AtomicBool::new(false);
static BADGE_RECT: [std::sync::atomic::AtomicI32; 4] = [const { std::sync::atomic::AtomicI32::new(0) }; 4];

fn held(vk: usize) -> bool {
    HELD[vk].load(Relaxed)
}
fn held_action(a: Action) -> bool {
    HELD_SC[cfg().key_of[a as usize]].load(Relaxed)
}
fn shift_phys() -> bool {
    held(LSHIFT) || held(RSHIFT)
}
fn ctrl_phys() -> bool {
    held(LCTRL) || held(RCTRL)
}
fn alt_phys() -> bool {
    held(LALT) || held(RALT)
}

/// Wakes the worker; it parks whenever nothing is moving or scrolling.
fn wake() {
    if let Some(t) = WORKER.get() {
        t.unpark();
    }
}

// ---- input synthesis ----
fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, size_of::<INPUT>() as i32);
    }
}

fn key_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

/// Left Shift by scan code (0x2A), the same key RShift is remapped to.
fn lshift(up: bool) {
    let f = KEYEVENTF_SCANCODE | if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    send(&[key_input(0, 0x2A, f)]);
}

fn click(down: MOUSE_EVENT_FLAGS, up: MOUSE_EVENT_FLAGS) {
    send(&[mouse_input(down, 0, 0, 0), mouse_input(up, 0, 0, 0)]);
}

/// Click without Shift: if Shift is held, lift it around the click so apps see
/// a plain click, then press it again.
fn click_unshifted(down: MOUSE_EVENT_FLAGS, up: MOUSE_EVENT_FLAGS) {
    if !shift_phys() {
        return click(down, up);
    }
    let sc = |up: bool| key_input(0, 0x2A, KEYEVENTF_SCANCODE | if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) });
    send(&[sc(true), mouse_input(down, 0, 0, 0), mouse_input(up, 0, 0, 0), sc(false)]);
}

fn tap_vk(vk: u16) {
    send(&[key_input(vk, 0, KEYBD_EVENT_FLAGS(0)), key_input(vk, 0, KEYEVENTF_KEYUP)]);
}

/// Extended-key tap (Home, End, ...), so apps don't see the numpad variant.
fn tap_ext(vk: u16) {
    send(&[key_input(vk, 0, KEYEVENTF_EXTENDEDKEY), key_input(vk, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP)]);
}

fn wheel(horizontal: bool, delta: i32) {
    let f = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
    send(&[mouse_input(f, 0, 0, delta)]);
}

/// Absolute move over the whole virtual desktop (real input event, no acceleration).
fn move_to(x: i32, y: i32) {
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN) as i64;
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN) as i64;
        let x = x.clamp(vx, vx + vw as i32 - 1);
        let y = y.clamp(vy, vy + vh as i32 - 1);
        // Ceil so Windows maps back to exactly this pixel.
        let nx = (((x - vx) as i64 * 65536 + vw - 1) / vw) as i32;
        let ny = (((y - vy) as i64 * 65536 + vh - 1) / vh) as i32;
        send(&[mouse_input(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny, 0)]);
    }
}

fn cursor() -> POINT {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    p
}

// ---- monitors ----
fn work_area(m: HMONITOR) -> RECT {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(m, &mut mi);
    }
    mi.rcWork
}

fn cursor_monitor() -> HMONITOR {
    unsafe { MonitorFromPoint(cursor(), MONITOR_DEFAULTTOPRIMARY) }
}

unsafe extern "system" fn collect_monitor(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let v = &mut *(data.0 as *mut Vec<HMONITOR>);
    v.push(m);
    BOOL(1)
}

/// All monitors, primary first.
fn monitors() -> Vec<HMONITOR> {
    let mut v: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect_monitor), LPARAM(&mut v as *mut _ as isize));
    }
    v.sort_by_key(|m| {
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = GetMonitorInfoW(*m, &mut mi);
        }
        mi.dwFlags & 1 == 0
    });
    v
}

fn jump_within(m: HMONITOR, pos: [f64; 2]) {
    let r = work_area(m);
    let x = r.left + ((r.right - r.left - 1) as f64 * pos[0]).round() as i32;
    let y = r.top + ((r.bottom - r.top - 1) as f64 * pos[1]).round() as i32;
    move_to(x, y);
}

fn jump_corner(index: usize) {
    jump_within(cursor_monitor(), cfg().jumps[index]);
}

fn jump_next_monitor() {
    let all = monitors();
    let cur = cursor_monitor();
    let i = all.iter().position(|m| m.0 == cur.0).unwrap_or(0);
    jump_within(all[(i + 1) % all.len()], [0.5, 0.5]);
}

// ---- grid mode ----
// A key opens a grid over the cursor's monitor. Each key press narrows to that
// cell and moves the cursor to its center; with Shift it also left-clicks there
// and closes. Enter/Esc/Caps close it; a click key closes it and clicks;
// Backspace steps back.
const GRID_COLS: i32 = 3;
/// Key presses after which the grid closes on its own.
const GRID_DEPTH: usize = 4;
/// Scan codes per grid row: 123 / QWE / ASD / ZXC. `grid_rows` 3 uses the last three.
const GRID_KEYS: [[usize; 3]; 4] = [
    [0x02, 0x03, 0x04],
    [0x10, 0x11, 0x12],
    [0x1E, 0x1F, 0x20],
    [0x2C, 0x2D, 0x2E],
];
const GRID_LABELS: [&[u8; 3]; 4] = [b"123", b"QWE", b"ASD", b"ZXC"];
static GRID: AtomicBool = AtomicBool::new(false);
static GRID_HWND: AtomicIsize = AtomicIsize::new(0);
/// Regions [left, top, right, bottom] in screen pixels; first = the monitor.
static GRID_STACK: Mutex<Vec<[i32; 4]>> = Mutex::new(Vec::new());

fn open_grid() {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(cursor_monitor(), &mut mi);
    }
    let r = mi.rcMonitor;
    *GRID_STACK.lock().unwrap() = vec![[r.left, r.top, r.right, r.bottom]];
    GRID.store(true, Relaxed);
    post(WM_GRID);
}

fn close_grid() {
    if GRID.swap(false, Relaxed) {
        GRID_STACK.lock().unwrap().clear();
        post(WM_GRID);
    }
}

fn grid_land(r: [i32; 4]) {
    move_to((r[0] + r[2] - 1) / 2, (r[1] + r[3] - 1) / 2);
}

/// `click`: land on the cell, then left-click there and close the grid.
fn grid_press(sc: usize, click: bool) {
    let rows = cfg().grid_rows;
    let off = 4 - rows;
    let Some((row, col)) =
        (0..rows).find_map(|r| GRID_KEYS[off + r].iter().position(|k| *k == sc).map(|c| (r as i32, c as i32)))
    else {
        close_grid(); // any other key leaves the grid, like G
        return;
    };
    let mut st = GRID_STACK.lock().unwrap();
    let Some(cur) = st.last().copied() else {
        return;
    };
    let (w, h) = (cur[2] - cur[0], cur[3] - cur[1]);
    if w < GRID_COLS * 2 || h < rows as i32 * 2 {
        drop(st);
        if click {
            close_grid(); // already pixel-sized: click where the cursor is
            click_unshifted(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP);
        }
        return;
    }
    let cell = |i: i32, n: i32, len: i32| (len as f64 * i as f64 / n as f64).round() as i32;
    let n = rows as i32;
    let next = [
        cur[0] + cell(col, GRID_COLS, w),
        cur[1] + cell(row, n, h),
        cur[0] + cell(col + 1, GRID_COLS, w),
        cur[1] + cell(row + 1, n, h),
    ];
    st.push(next);
    let done = st.len() > GRID_DEPTH; // root + GRID_DEPTH presses
    drop(st);
    grid_land(next);
    if click {
        close_grid();
        click_unshifted(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP);
    } else if done {
        close_grid();
    } else {
        post(WM_GRID);
    }
}

fn grid_back() {
    let mut st = GRID_STACK.lock().unwrap();
    if st.len() > 1 {
        st.pop();
        let r = *st.last().unwrap();
        drop(st);
        grid_land(r);
        post(WM_GRID);
    } else {
        drop(st);
        close_grid();
    }
}

fn update_grid() {
    let h = HWND(GRID_HWND.load(Relaxed) as *mut _);
    unsafe {
        if GRID.load(Relaxed) {
            let Some(o) = GRID_STACK.lock().unwrap().first().copied() else {
                return;
            };
            let _ = SetWindowPos(
                h, Some(HWND_TOPMOST), o[0], o[1], o[2] - o[0], o[3] - o[1],
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(h), None, false);
        } else {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

const GRID_KEY_COLOR: COLORREF = COLORREF(0x030201); // transparent via color key

fn paint_grid(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let keyb = CreateSolidBrush(GRID_KEY_COLOR);
        FillRect(dc, &rc, keyb);
        let _ = DeleteObject(keyb.into());

        let (o, c) = {
            let st = GRID_STACK.lock().unwrap();
            match (st.first(), st.last()) {
                (Some(o), Some(c)) => (*o, *c),
                _ => {
                    let _ = EndPaint(hwnd, &ps);
                    return;
                }
            }
        };
        let r = [c[0] - o[0], c[1] - o[1], c[2] - o[0], c[3] - o[1]];
        let rows = cfg().grid_rows as i32;
        let off = 4 - rows as usize;
        let (w, h) = (r[2] - r[0], r[3] - r[1]);

        let pen = CreatePen(PS_SOLID, px(2), rgb(0xF2, 0xA3, 0x3A));
        let old_pen = SelectObject(dc, pen.into());
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let _ = Rectangle(dc, r[0], r[1], r[2], r[3]);
        for i in 1..GRID_COLS {
            let x = r[0] + w * i / GRID_COLS;
            let _ = MoveToEx(dc, x, r[1], None);
            let _ = LineTo(dc, x, r[3]);
        }
        for j in 1..rows {
            let y = r[1] + h * j / rows;
            let _ = MoveToEx(dc, r[0], y, None);
            let _ = LineTo(dc, r[2], y);
        }
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(pen.into());

        // Key labels, skipped once cells get too small to hold them.
        let (cw, ch) = (w / GRID_COLS, h / rows);
        if cw >= px(16) && ch >= px(16) {
            let font = CreateFontW(
                -px(14), 0, 0, 0, 700, 0, 0, 0,
                FONT_CHARSET(1), FONT_OUTPUT_PRECISION(0), FONT_CLIP_PRECISION(0),
                FONT_QUALITY(5), 0, w!("Segoe UI"),
            );
            let old_font = SelectObject(dc, font.into());
            let bg = CreateSolidBrush(rgb(0x17, 0x21, 0x2B));
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, rgb(0x76, 0xE4, 0xAD));
            let (pw, ph) = (cw.min(px(26)) - 2, ch.min(px(22)) - 2);
            for row in 0..rows {
                for col in 0..GRID_COLS {
                    let cx = r[0] + w * (2 * col + 1) / (2 * GRID_COLS);
                    let cy = r[1] + h * (2 * row + 1) / (2 * rows);
                    let mut pill = RECT { left: cx - pw / 2, top: cy - ph / 2, right: cx + pw / 2, bottom: cy + ph / 2 };
                    FillRect(dc, &pill, bg);
                    let ch = GRID_LABELS[off + row as usize][col as usize] as u16;
                    DrawTextW(dc, &mut [ch], &mut pill, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                }
            }
            SelectObject(dc, old_font);
            let _ = DeleteObject(bg.into());
            let _ = DeleteObject(font.into());
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn grid_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_grid(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn create_grid() {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let class = w!("kbmouse_grid");
        let wc = WNDCLASSW { lpfnWndProc: Some(grid_proc), hInstance: hinst, lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            class, w!("Keyboard mouse grid"), WS_POPUP, 0, 0, 1, 1, None, None, Some(hinst), None,
        )
        .unwrap();
        let _ = SetLayeredWindowAttributes(hwnd, GRID_KEY_COLOR, 230, LWA_COLORKEY | LWA_ALPHA);
        GRID_HWND.store(hwnd.0 as isize, Relaxed);
    }
}

// ---- hint mode ----
// F scans the focused window with UI Automation for clickable controls and
// labels each with 1-2 letters. Typing a label moves the cursor there and
// clicks; with Shift it only moves (e.g. to then nudge along a seek bar).
// Enter/Esc/Caps/F close it (the hint key is never used in a label); Backspace erases a typed letter.
const HINT_LETTERS: &[u8; 26] = b"ASDFGHJKLQWERTYUIOPZXCVBNM";

/// Label alphabet: every letter except the one bound to the hint key.
fn hint_alphabet() -> Vec<u8> {
    let hint_sc = cfg().key_of[Hint as usize];
    HINT_LETTERS.iter().copied().filter(|c| key_sc(&(*c as char).to_string()) != Some(hint_sc)).collect()
}

/// Fixed label letters for the window buttons: Q close, W maximize, E minimize.
const CAPTION_KEYS: [u8; 3] = *b"QWE";

/// Rects of the window's close / maximize / minimize buttons as (role, rect).
/// The frame draws them, so terminals and custom title bars often don't expose
/// them to UI Automation.
fn caption_slots(fg: isize) -> Vec<(usize, [i32; 4])> {
    let mut out = Vec::new();
    unsafe {
        let hwnd = HWND(fg as *mut _);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        if style & WS_SYSMENU.0 == 0 || style & WS_CAPTION.0 != WS_CAPTION.0 {
            return out;
        }
        let mut f = RECT::default();
        if DwmGetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(9), &mut f as *mut _ as *mut _, size_of::<RECT>() as u32).is_err() {
            return out;
        }
        let dpi = GetDpiForWindow(hwnd) as i32;
        let (bw, bh) = (46 * dpi / 96, 32 * dpi / 96);
        let mut right = f.right;
        for (role, on) in [(0, true), (1, style & WS_MAXIMIZEBOX.0 != 0), (2, style & WS_MINIMIZEBOX.0 != 0)] {
            if on {
                out.push((role, [right - bw, f.top, right, f.top + bh]));
                right -= bw;
            }
        }
    }
    out
}

struct Hint {
    rect: [i32; 4],
    label: String,
}
static HINT: AtomicBool = AtomicBool::new(false);
static HINT_GEN: AtomicUsize = AtomicUsize::new(0);
static HINT_HWND: AtomicIsize = AtomicIsize::new(0);
static HINTS: Mutex<Vec<Hint>> = Mutex::new(Vec::new());
static HINT_TYPED: Mutex<String> = Mutex::new(String::new());

/// Scans the window and labels controls. Window buttons get fixed letters
/// (CAPTION_KEYS); other controls get the first letter of their name when that
/// is unambiguous (two letters, name initial first, when several share it), and
/// the remaining free letters otherwise. Labels never prefix one another.
fn enumerate_hints(fg: isize) -> Vec<Hint> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let found = (|| -> windows::core::Result<Vec<([i32; 4], String)>> {
            let ua: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = ua.ElementFromHandle(HWND(fg as *mut _))?;
            let cache = ua.CreateCacheRequest()?;
            cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
            cache.AddProperty(UIA_NamePropertyId)?;
            let prop = |id, v: bool| ua.CreatePropertyCondition(id, &VARIANT::from(v));
            let mut any = prop(UIA_IsInvokePatternAvailablePropertyId, true)?;
            for id in [
                UIA_IsTogglePatternAvailablePropertyId,
                UIA_IsSelectionItemPatternAvailablePropertyId,
                UIA_IsExpandCollapsePatternAvailablePropertyId,
                UIA_IsRangeValuePatternAvailablePropertyId,
                UIA_IsValuePatternAvailablePropertyId,
            ] {
                any = ua.CreateOrCondition(&any, &prop(id, true)?)?;
            }
            // Buttons and friends even when they expose no action pattern.
            for ct in [
                UIA_ButtonControlTypeId, UIA_TabItemControlTypeId, UIA_MenuItemControlTypeId,
                UIA_CheckBoxControlTypeId, UIA_RadioButtonControlTypeId, UIA_HyperlinkControlTypeId,
                UIA_SplitButtonControlTypeId, UIA_ListItemControlTypeId, UIA_TreeItemControlTypeId,
            ] {
                let c = ua.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(ct.0))?;
                any = ua.CreateOrCondition(&any, &c)?;
            }
            let cond = ua.CreateAndCondition(&any, &prop(UIA_IsOffscreenPropertyId, false)?)?;
            let cond = ua.CreateAndCondition(&cond, &prop(UIA_IsEnabledPropertyId, true)?)?;
            let all = root.FindAllBuildCache(TreeScope_Descendants, &cond, &cache)?;
            let mut v = Vec::new();
            for i in 0..all.Length()? {
                let Ok(e) = all.GetElement(i) else { continue };
                if let Ok(r) = e.CachedBoundingRectangle() {
                    if r.right > r.left && r.bottom > r.top {
                        let name = e.CachedName().map(|n| n.to_string()).unwrap_or_default();
                        v.push(([r.left, r.top, r.right, r.bottom], name));
                    }
                }
            }
            Ok(v)
        })();
        let mut items = found.unwrap_or_default();
        items.sort_by_key(|(r, _)| (r[1], r[0]));
        items.dedup_by_key(|(r, _)| ((r[0] + r[2]) / 2, (r[1] + r[3]) / 2));

        let all_letters = hint_alphabet();
        let reserved: Vec<u8> = CAPTION_KEYS.iter().copied().filter(|c| all_letters.contains(c)).collect();
        let abc: Vec<u8> = all_letters.iter().copied().filter(|c| !reserved.contains(c)).collect();
        let l = abc.len();
        let mut out: Vec<Hint> = Vec::new();

        // Window buttons: fixed letters; drop any UIA duplicate of the same button.
        for (role, b) in caption_slots(fg) {
            let key = CAPTION_KEYS[role];
            if !reserved.contains(&key) {
                continue;
            }
            let (cx, cy) = ((b[0] + b[2]) / 2, (b[1] + b[3]) / 2);
            let (hw, hh) = ((b[2] - b[0]) / 2, b[3] - b[1]);
            items.retain(|(r, _)| ((r[0] + r[2]) / 2 - cx).abs() >= hw || ((r[1] + r[3]) / 2 - cy).abs() >= hh);
            out.push(Hint { rect: b, label: (key as char).to_string() });
        }
        if l == 0 {
            return out;
        }

        // Group the rest by the initial of their name.
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); l];
        let mut rest: Vec<usize> = Vec::new();
        for (i, (_, name)) in items.iter().enumerate() {
            let init = name.trim().chars().next().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase() as u8);
            match init.and_then(|c| abc.iter().position(|a| *a == c)) {
                Some(g) => groups[g].push(i),
                None => rest.push(i),
            }
        }
        let mut labels: Vec<Option<String>> = vec![None; items.len()];
        let mut used = vec![false; l];
        for (g, mem) in groups.iter().enumerate() {
            if mem.is_empty() {
                continue;
            }
            used[g] = true;
            if mem.len() == 1 {
                labels[mem[0]] = Some((abc[g] as char).to_string());
            } else {
                for (k, &m) in mem.iter().take(l).enumerate() {
                    labels[m] = Some(format!("{}{}", abc[g] as char, abc[k] as char));
                }
                rest.extend(mem.iter().skip(l));
            }
        }
        rest.sort_unstable();
        let free: Vec<usize> = (0..l).filter(|g| !used[*g]).collect();
        for (j, &m) in rest.iter().enumerate() {
            labels[m] = if rest.len() <= free.len() {
                Some((abc[free[j]] as char).to_string())
            } else {
                free.get(j / l).map(|f| format!("{}{}", abc[*f] as char, abc[j % l] as char))
            };
        }
        for ((rect, _), label) in items.into_iter().zip(labels) {
            if let Some(label) = label {
                out.push(Hint { rect, label });
            }
        }
        out
    }
}

fn open_hint() {
    close_grid();
    // Prefer the window the foreground event reported: right after Alt+Tab,
    // GetForegroundWindow can still return the window just left.
    let ev = FG_EVENT.load(Relaxed);
    let fg = if ev != 0 && unsafe { IsWindow(Some(HWND(ev as *mut _))) }.as_bool() {
        ev
    } else {
        unsafe { GetForegroundWindow() }.0 as isize
    };
    let gen = HINT_GEN.fetch_add(1, Relaxed) + 1;
    HINTS.lock().unwrap().clear();
    HINT_TYPED.lock().unwrap().clear();
    HINT.store(true, Relaxed);
    // UI Automation can be slow; keep it off the hook thread.
    std::thread::spawn(move || {
        let list = enumerate_hints(fg);
        if HINT.load(Relaxed) && HINT_GEN.load(Relaxed) == gen {
            *HINTS.lock().unwrap() = list;
            post(WM_HINT);
        }
    });
}

fn close_hint() {
    if HINT.swap(false, Relaxed) {
        HINT_GEN.fetch_add(1, Relaxed);
        HINTS.lock().unwrap().clear();
        post(WM_HINT);
    }
}

fn hint_key(vk: usize, act: Option<Action>, shift: bool) {
    if matches!(vk, 0x0D | 0x1B) || act == Some(Hint) {
        return close_hint();
    }
    if vk == 0x08 {
        HINT_TYPED.lock().unwrap().pop();
        return post(WM_HINT);
    }
    if !(0x41..=0x5A).contains(&vk) {
        return close_hint();
    }
    let mut typed = HINT_TYPED.lock().unwrap();
    typed.push(vk as u8 as char);
    let hints = HINTS.lock().unwrap();
    let hit = hints.iter().find(|h| h.label == *typed).map(|h| h.rect);
    let any = hints.iter().any(|h| h.label.starts_with(typed.as_str()));
    drop(hints);
    if !any {
        typed.pop(); // no such label: ignore the key
        return;
    }
    drop(typed);
    match hit {
        Some(r) => {
            close_hint();
            move_to((r[0] + r[2] - 1) / 2, (r[1] + r[3] - 1) / 2);
            if !shift {
                click(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP);
            }
        }
        None => post(WM_HINT),
    }
}

fn virtual_rect() -> [i32; 4] {
    unsafe {
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        [x, y, GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)]
    }
}

fn update_hint() {
    let h = HWND(HINT_HWND.load(Relaxed) as *mut _);
    unsafe {
        if HINT.load(Relaxed) && !HINTS.lock().unwrap().is_empty() {
            let [x, y, w, hh] = virtual_rect();
            let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, w, hh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            let _ = InvalidateRect(Some(h), None, false);
        } else {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

fn paint_hint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let keyb = CreateSolidBrush(GRID_KEY_COLOR);
        FillRect(dc, &rc, keyb);
        let _ = DeleteObject(keyb.into());

        let font = CreateFontW(
            -px(13), 0, 0, 0, 700, 0, 0, 0,
            FONT_CHARSET(1), FONT_OUTPUT_PRECISION(0), FONT_CLIP_PRECISION(0),
            FONT_QUALITY(5), 0, w!("Segoe UI"),
        );
        let old_font = SelectObject(dc, font.into());
        let bg = CreateSolidBrush(rgb(0x17, 0x21, 0x2B));
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, rgb(0xF2, 0xA3, 0x3A));
        let [vx, vy, _, _] = virtual_rect();
        let typed = HINT_TYPED.lock().unwrap().clone();
        for hint in HINTS.lock().unwrap().iter() {
            if !hint.label.starts_with(&typed) {
                continue;
            }
            let mut rest: Vec<u16> = hint.label[typed.len()..].encode_utf16().collect();
            let (pw, ph) = (px(9) * rest.len() as i32 + px(8), px(18));
            let (x, y) = (hint.rect[0] - vx, hint.rect[1] - vy);
            let mut pill = RECT { left: x, top: y, right: x + pw, bottom: y + ph };
            FillRect(dc, &pill, bg);
            DrawTextW(dc, &mut rest, &mut pill, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        }
        SelectObject(dc, old_font);
        let _ = DeleteObject(bg.into());
        let _ = DeleteObject(font.into());
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn hint_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_hint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn create_hint() {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let class = w!("kbmouse_hint");
        let wc = WNDCLASSW { lpfnWndProc: Some(hint_proc), hInstance: hinst, lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            class, w!("Keyboard mouse hints"), WS_POPUP, 0, 0, 1, 1, None, None, Some(hinst), None,
        )
        .unwrap();
        let _ = SetLayeredWindowAttributes(hwnd, GRID_KEY_COLOR, 235, LWA_COLORKEY | LWA_ALPHA);
        HINT_HWND.store(hwnd.0 as isize, Relaxed);
    }
}

// ---- mode / drag / speed ----
fn post(msg: u32) {
    let h = BADGE.load(Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(h as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

fn set_mode(on: bool) {
    MODE.store(on, Relaxed);
    for s in &SCROLL {
        s.store(false, Relaxed);
    }
    RESET.store(true, Relaxed);
    if !on {
        release_drag();
        close_grid();
        close_hint();
    }
    post(WM_MODE);
    wake();
}

fn toggle_mode() {
    set_mode(!MODE.load(Relaxed));
}

/// Session locked or unlocked: key-ups on the secure desktop never reach the
/// hook, so drop mouse mode, the drag button and all tracked key state.
fn release_all() {
    if held(RSHIFT) {
        lshift(true);
    }
    for i in 0..256 {
        HELD[i].store(false, Relaxed);
        HELD_SC[i].store(false, Relaxed);
        SWALLOWED[i].store(false, Relaxed);
        HINT_USED[i].store(false, Relaxed);
    }
    set_mode(false);
}

fn toggle_drag() {
    let on = !DRAGGING.load(Relaxed);
    DRAGGING.store(on, Relaxed);
    send(&[mouse_input(if on { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP }, 0, 0, 0)]);
    post(WM_REFRESH);
}

fn release_drag() {
    if DRAGGING.swap(false, Relaxed) {
        send(&[mouse_input(MOUSEEVENTF_LEFTUP, 0, 0, 0)]);
    }
}

fn change_speed(delta: isize) {
    let lvl = (SPEED_LEVEL.load(Relaxed) as isize + delta).clamp(1, cfg().speeds.len() as isize);
    SPEED_LEVEL.store(lvl as usize, Relaxed);
    post(WM_REFRESH);
    post(WM_SAVE_SOON);
}

// ---- per-app disable ----
fn exe_name(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        ok.then(|| path.rsplit('\\').next().unwrap_or("").to_lowercase())
    }
}

fn check_foreground() {
    let hit = exe_name(unsafe { GetForegroundWindow() }).is_some_and(|n| cfg().disabled_apps.contains(&n));
    if DISABLED.swap(hit, Relaxed) != hit && hit {
        set_mode(false); // also releases drag and closes the grid
    }
}

/// Window that most recently became foreground, as reported by the event hook.
static FG_EVENT: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, hwnd: HWND, _: i32, _: i32, _: u32, _: u32) {
    FG_EVENT.store(hwnd.0 as isize, Relaxed);
    check_foreground();
}

// ---- keyboard hook ----
unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // Skip events we (or other tools) injected.
        if k.flags.0 & 0x10 == 0 {
            let swallow = handle_key(k);
            // Any press or release can start, stop or retarget motion
            // (opposite keys, Ctrl, Space, Shift), so let the worker re-check.
            if MODE.load(Relaxed) {
                wake();
            }
            if swallow {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Returns true to swallow the key.
fn handle_key(k: &KBDLLHOOKSTRUCT) -> bool {
    let vk = k.vkCode as usize;
    if vk > 255 {
        return false;
    }
    let up = k.flags.0 & 0x80 != 0;
    let repeat = HELD[vk].swap(!up, Relaxed);
    // Mouse-layer keys are bound by physical position (scan code); extended
    // keys (arrows, right Ctrl, ...) share scan codes with others, so skip them.
    let sc = k.scanCode as usize;
    let act = if k.flags.0 & 1 == 0 && sc < 256 {
        HELD_SC[sc].store(!up, Relaxed);
        cfg().action_at[sc]
    } else {
        None
    };

    if HINT_USED[vk].load(Relaxed) {
        if up {
            HINT_USED[vk].store(false, Relaxed);
        } else {
            if k.flags.0 & 1 == 0 && sc < 256 {
                HELD_SC[sc].store(false, Relaxed);
            }
            SWALLOWED[vk].store(true, Relaxed);
            return true;
        }
    }

    if up {
        match act {
            Some(ScrollUp) => SCROLL[0].store(false, Relaxed),
            Some(ScrollDown) => SCROLL[1].store(false, Relaxed),
            Some(ScrollLeft) => SCROLL[2].store(false, Relaxed),
            Some(ScrollRight) => SCROLL[3].store(false, Relaxed),
            _ => {}
        }
    }

    // Right Shift acts as Left Shift.
    if vk == RSHIFT {
        if !(SCROLLING.load(Relaxed) && !up) {
            lshift(up);
        } else {
            SHIFT_HIDDEN.store(true, Relaxed);
        }
        return true;
    }
    if vk == LSHIFT && !up && SCROLLING.load(Relaxed) {
        SHIFT_HIDDEN.store(true, Relaxed);
        return true;
    }

    // Disabled app focused: everything passes through, including Caps Lock.
    if DISABLED.load(Relaxed) {
        return up && SWALLOWED[vk].swap(false, Relaxed);
    }

    // Caps Lock: toggle mouse mode. Shift+Caps toggles the real Caps Lock.
    if vk == CAPS {
        if !up && !repeat {
            if shift_phys() {
                tap_vk(VK_CAPITAL.0);
            } else {
                toggle_mode();
            }
        }
        return true;
    }

    // Ctrl+Alt+Q quits.
    if vk == KEY_Q && !up && ctrl_phys() && alt_phys() {
        SWALLOWED[vk].store(true, Relaxed);
        unsafe { PostQuitMessage(0) };
        return true;
    }

    if up {
        return SWALLOWED[vk].swap(false, Relaxed);
    }
    if !MODE.load(Relaxed) {
        return false;
    }

    // Grid open: keys drive the grid; modifiers pass through.
    if GRID.load(Relaxed) {
        if matches!(vk, 0xA0..=0xA5 | 0x10..=0x12 | 0x5B | 0x5C) {
            return false;
        }
        if !repeat {
            match (vk, act) {
                (0x0D | 0x1B, _) => close_grid(),
                (_, Some(Grid)) => open_grid(), // restart: full grid on the cursor's monitor
                (0x08, _) => grid_back(),
                (_, Some(ClickLeft)) => {
                    close_grid();
                    click(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP);
                }
                (_, Some(ClickRight)) => {
                    close_grid();
                    click(MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP);
                }
                (_, Some(ClickMiddle)) => {
                    close_grid();
                    click(MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP);
                }
                _ => {
                    if k.flags.0 & 1 == 0 {
                        grid_press(sc, shift_phys());
                    } else {
                        close_grid();
                    }
                }
            }
        }
        SWALLOWED[vk].store(true, Relaxed);
        return true;
    }

    // Hint mode: letters pick a label; modifiers pass through.
    if HINT.load(Relaxed) {
        if matches!(vk, 0xA0..=0xA5 | 0x10..=0x12 | 0x5B | 0x5C) {
            return false;
        }
        if !repeat {
            hint_key(vk, act, shift_phys());
        }
        HINT_USED[vk].store(true, Relaxed);
        if k.flags.0 & 1 == 0 && sc < 256 {
            HELD_SC[sc].store(false, Relaxed);
        }
        SWALLOWED[vk].store(true, Relaxed);
        return true;
    }

    let Some(act) = act else {
        return false;
    };

    match act {
        // Movement is driven by the worker from HELD_SC; Slow is a modifier.
        Up | Down | Left | Right | Slow => {}
        ClickRight => click(MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        ClickMiddle => click(MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        ClickLeft => click(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        Drag => {
            if !repeat {
                toggle_drag();
            }
        }
        ScrollUp | ScrollDown | ScrollLeft | ScrollRight => scroll_key(act),
        SpeedUp => change_speed(1),
        SpeedDown => change_speed(-1),
        // Back/Forward, except with Ctrl so Ctrl+V / Ctrl+B pass through.
        Back | Forward => {
            if ctrl_phys() {
                return false;
            }
            tap_vk(if act == Forward { VK_BROWSER_FORWARD.0 } else { VK_BROWSER_BACK.0 });
        }
        // Home / End auto-repeat; Shift and Ctrl still apply.
        Home => tap_ext(VK_HOME.0),
        End => tap_ext(VK_END.0),
        Corner1 | Corner2 | Corner3 | Corner4 => {
            if !repeat {
                jump_corner(act as usize - Corner1 as usize);
            }
        }
        NextMonitor => {
            if !repeat {
                jump_next_monitor();
            }
        }
        Grid => {
            if !repeat {
                open_grid();
            }
        }
        Hint => {
            if !repeat {
                open_hint();
            }
        }
    }
    SWALLOWED[vk].store(true, Relaxed);
    true
}

fn scroll_key(act: Action) {
    if ctrl_phys() {
        // One whole notch per press so Ctrl+wheel zooms in steps.
        match act {
            ScrollUp => wheel(false, 120),
            ScrollDown => wheel(false, -120),
            ScrollRight => wheel(true, 120),
            _ => wheel(true, -120),
        }
        return;
    }
    let i = match act {
        ScrollUp => 0,
        ScrollDown => 1,
        ScrollLeft => 2,
        _ => 3,
    };
    SCROLL[i].store(true, Relaxed);
}

// ---- worker: cursor movement + smooth scroll ----
struct Worker {
    last: Instant,
    moving: bool,
    fx: f64,
    fy: f64,
    scrolling: bool,
    scroll_last: Instant,
    sfv: f64,
    sfh: f64,
}

impl Worker {
    fn stop_scroll(&mut self) {
        if !self.scrolling {
            return;
        }
        self.scrolling = false;
        SCROLLING.store(false, Relaxed);
        // Give Shift back to the app if it is still physically held.
        if SHIFT_HIDDEN.swap(false, Relaxed) && shift_phys() {
            lshift(false);
        }
    }

    fn stop_move(&mut self) {
        self.moving = false;
        self.fx = 0.0;
        self.fy = 0.0;
    }

    fn tick(&mut self) -> bool {
        let mode = MODE.load(Relaxed);
        if RESET.swap(false, Relaxed) || !mode {
            self.stop_scroll();
            self.stop_move();
            if !mode {
                return false;
            }
        }
        let now = Instant::now();
        let c = cfg();
        let space = held_action(Slow);
        let boost = if shift_phys() { c.shift_boost } else { 1.0 };
        let mut active = false;

        // Cursor movement.
        let grid = GRID.load(Relaxed) || HINT.load(Relaxed);
        let dx = if grid { 0 } else { held_action(Right) as i32 - held_action(Left) as i32 };
        let dy = if grid { 0 } else { held_action(Down) as i32 - held_action(Up) as i32 };
        if dx != 0 || dy != 0 {
            active = true;
            if !self.moving {
                self.moving = true;
                self.last = now;
            }
            let dt = (now - self.last).as_secs_f64().clamp(0.001, 0.05);
            self.last = now;
            let vel = c.speeds[SPEED_LEVEL.load(Relaxed) - 1] * if space { c.slow_move } else { 1.0 } * boost;
            let (mut ax, mut ay) = (dx as f64, dy as f64);
            if dx != 0 && dy != 0 {
                ax *= 0.70710678;
                ay *= 0.70710678;
            }
            self.fx += ax * vel * dt;
            self.fy += ay * vel * dt;
            let (mx, my) = (self.fx.round(), self.fy.round());
            self.fx -= mx;
            self.fy -= my;
            if mx != 0.0 || my != 0.0 {
                let p = cursor();
                move_to(p.x + mx as i32, p.y + my as i32);
            }
        } else {
            self.stop_move();
        }

        // Smooth scroll.
        let v = SCROLL[0].load(Relaxed) as i32 - SCROLL[1].load(Relaxed) as i32;
        let h = SCROLL[3].load(Relaxed) as i32 - SCROLL[2].load(Relaxed) as i32;
        if (v != 0 || h != 0) && !ctrl_phys() {
            active = true;
            if !self.scrolling {
                self.scrolling = true;
                self.scroll_last = now;
                self.sfv = 0.0;
                self.sfh = 0.0;
                SCROLLING.store(true, Relaxed);
                // Apps treat Shift+wheel as horizontal; hide Shift while scrolling.
                if shift_phys() {
                    SHIFT_HIDDEN.store(true, Relaxed);
                    lshift(true);
                }
            }
            let dt = (now - self.scroll_last).as_secs_f64().min(0.05);
            self.scroll_last = now;
            let vel = c.scroll_speed * if space { c.slow_scroll } else { 1.0 } * boost;
            self.sfv += v as f64 * vel * dt;
            self.sfh += h as f64 * vel * dt;
            let dv = self.sfv.round();
            if dv != 0.0 {
                self.sfv -= dv;
                wheel(false, dv as i32);
            }
            let dh = self.sfh.round();
            if dh != 0.0 {
                self.sfh -= dh;
                wheel(true, dh as i32);
            }
        } else {
            self.stop_scroll();
        }
        active
    }
}

fn worker() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
    let now = Instant::now();
    let mut w = Worker {
        last: now,
        moving: false,
        fx: 0.0,
        fy: 0.0,
        scrolling: false,
        scroll_last: now,
        sfv: 0.0,
        sfh: 0.0,
    };
    loop {
        if w.tick() {
            std::thread::sleep(TICK);
        } else {
            std::thread::park();
        }
    }
}

// ---- badge window ----
fn scale() -> i32 {
    unsafe { GetDpiForSystem() as i32 }
}
fn px(v: i32) -> i32 {
    v * scale() / 96
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// Badge rect [x, y, w, h] for a monitor: bottom-left of the work area, moved
/// above any taskbar overlapping it (an auto-hidden taskbar slides over the
/// work area and would otherwise cover the badge).
fn badge_rect(mon: HMONITOR) -> [i32; 4] {
    let r = work_area(mon);
    let (w, h) = (px(50), px(26));
    let (x, mut y) = (r.left + px(16), r.bottom - h - px(16));
    unsafe {
        let mut bar = FindWindowW(w!("Shell_TrayWnd"), None).ok();
        let mut secondary = false;
        while let Some(t) = bar {
            let mut tr = RECT::default();
            if IsWindowVisible(t).as_bool() && GetWindowRect(t, &mut tr).is_ok() {
                let overlaps = tr.left < x + w && tr.right > x && tr.top < y + h && tr.bottom > y;
                if overlaps {
                    y = tr.top - h - px(16);
                }
            }
            if !secondary {
                secondary = true;
                bar = FindWindowExW(None, None, w!("Shell_SecondaryTrayWnd"), None).ok();
            } else {
                bar = FindWindowExW(None, Some(t), w!("Shell_SecondaryTrayWnd"), None).ok();
            }
        }
    }
    [x, y, w, h]
}

fn update_badge(hwnd: HWND) {
    unsafe {
        let mon = cursor_monitor();
        BADGE_MONITOR.store(mon.0 as isize, Relaxed);
        let rect = badge_rect(mon);
        let [x, y, w, h] = rect;
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        for (i, v) in rect.into_iter().enumerate() {
            BADGE_RECT[i].store(v, Relaxed);
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    refresh_visibility(hwnd);
}

/// Follows the cursor's monitor; hides while the cursor is over the badge.
fn refresh_visibility(hwnd: HWND) {
    if !MODE.load(Relaxed) {
        return;
    }
    let mon = cursor_monitor();
    // Also re-place when the taskbar shows or hides.
    if mon.0 as isize != BADGE_MONITOR.load(Relaxed)
        || badge_rect(mon) != [0, 1, 2, 3].map(|i| BADGE_RECT[i].load(Relaxed))
    {
        update_badge(hwnd);
        return;
    }
    let p = cursor();
    let [x, y, w, h] = [0, 1, 2, 3].map(|i| BADGE_RECT[i].load(Relaxed));
    let over = p.x >= x && p.x < x + w && p.y >= y && p.y < y + h;
    if (!over) != BADGE_VISIBLE.load(Relaxed) {
        unsafe {
            let _ = ShowWindow(hwnd, if over { SW_HIDE } else { SW_SHOWNOACTIVATE });
        }
        BADGE_VISIBLE.store(!over, Relaxed);
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let bg = CreateSolidBrush(rgb(0x17, 0x21, 0x2B));
        FillRect(dc, &rc, bg);
        let _ = DeleteObject(bg.into());
        SetBkMode(dc, TRANSPARENT);

        let font = |size: i32, weight: i32, face: PCWSTR| {
            CreateFontW(
                -px(size), 0, 0, 0, weight, 0, 0, 0,
                FONT_CHARSET(1), FONT_OUTPUT_PRECISION(0), FONT_CLIP_PRECISION(0),
                FONT_QUALITY(5), 0, face,
            )
        };
        let fmt = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX;

        // Mouse glyph; amber while dragging.
        let icon = font(14, 400, w!("Segoe Fluent Icons"));
        let old = SelectObject(dc, icon.into());
        SetTextColor(dc, if DRAGGING.load(Relaxed) { rgb(0xF2, 0xA3, 0x3A) } else { rgb(0x76, 0xE4, 0xAD) });
        let mut r1 = RECT { left: px(8), top: 0, right: px(30), bottom: rc.bottom };
        DrawTextW(dc, &mut [0xE962u16], &mut r1, fmt);

        // Speed level.
        let txt = font(14, 700, w!("Segoe UI"));
        SelectObject(dc, txt.into());
        let mut r2 = RECT { left: px(33), top: 0, right: rc.right, bottom: rc.bottom };
        let digit = [b'0' as u16 + SPEED_LEVEL.load(Relaxed) as u16];
        DrawTextW(dc, &mut digit.clone(), &mut r2, fmt);

        SelectObject(dc, old);
        let _ = DeleteObject(icon.into());
        let _ = DeleteObject(txt.into());
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn badge_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_REFRESH => {
            if MODE.load(Relaxed) {
                update_badge(hwnd);
            }
            LRESULT(0)
        }
        WM_MODE => {
            if MODE.load(Relaxed) {
                BADGE_VISIBLE.store(false, Relaxed);
                update_badge(hwnd);
                SetTimer(Some(hwnd), TIMER_BADGE, 50, None);
            } else {
                let _ = KillTimer(Some(hwnd), TIMER_BADGE);
                let _ = ShowWindow(hwnd, SW_HIDE);
                BADGE_VISIBLE.store(false, Relaxed);
            }
            LRESULT(0)
        }
        WM_GRID => {
            update_grid();
            LRESULT(0)
        }
        WM_HINT => {
            update_hint();
            LRESULT(0)
        }
        WM_SAVE_SOON => {
            // Debounce: write the state file once the speed stops changing.
            SetTimer(Some(hwnd), TIMER_SAVE, 500, None);
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_SAVE {
                let _ = KillTimer(Some(hwnd), TIMER_SAVE);
                save_state();
                return LRESULT(0);
            }
            refresh_visibility(hwnd);
            LRESULT(0)
        }
        WM_WTSSESSION_CHANGE => {
            if matches!(wparam.0 as u32, WTS_SESSION_LOCK | WTS_SESSION_UNLOCK) {
                release_all();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn create_badge() -> HWND {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let class = w!("kbmouse_badge");
        let wc = WNDCLASSW { lpfnWndProc: Some(badge_proc), hInstance: hinst, lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        // Click-through, always on top, never takes focus.
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            class, w!("Keyboard mouse"), WS_POPUP, 0, 0, px(50), px(26), None, None, Some(hinst), None,
        )
        .unwrap();
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 190, LWA_ALPHA);
        let round = 2i32; // DWMWCP_ROUND
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(33), &round as *const _ as *const _, 4);
        BADGE.store(hwnd.0 as isize, Relaxed);
        hwnd
    }
}

fn main() {
    unsafe {
        // One instance only.
        let mutex = CreateMutexW(None, true, w!("Local\\kbmouse-rs"));
        if mutex.is_err() || GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);

        let mut warns = Vec::new();
        let _ = CFG.set(load_config(&mut warns));
        SPEED_LEVEL.store(load_state(), Relaxed);
        if !warns.is_empty() {
            warn_box(&warns.join("\n"));
        }

        // Real Caps Lock starts off; Caps is swallowed from here on.
        if GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 {
            tap_vk(VK_CAPITAL.0);
        }

        let hwnd = create_badge();
        create_grid();
        create_hint();
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), Some(hinst), 0).unwrap();
        let winhook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT,
        );
        check_foreground();
        let t = std::thread::spawn(worker);
        let _ = WORKER.set(t.thread().clone());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        release_drag();
        close_grid();
        close_hint();
        save_state();
        let _ = UnhookWinEvent(winhook);
        let _ = UnhookWindowsHookEx(hook);
        let _ = WTSUnRegisterSessionNotification(hwnd);
        let _ = DestroyWindow(hwnd);
    }
}
