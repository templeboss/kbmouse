#![windows_subsystem = "windows"]

//! Keyboard-driven mouse. Caps Lock toggles a mouse layer (see README.md).
//! A low-level keyboard hook swallows layer keys; a worker thread drives
//! cursor movement and smooth scrolling; a small layered window is the badge.

use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering::Relaxed};
use std::sync::OnceLock;
use std::thread::Thread;
use std::time::{Duration, Instant};

use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ---- tuning (same values as kbmouse.ahk) ----
const SPEEDS: [f64; 5] = [1800.0, 2400.0, 3000.0, 4000.0, 5200.0];
const JUMPS: [[f64; 2]; 4] = [[0.10, 0.10], [0.90, 0.10], [0.10, 0.90], [0.90, 0.90]];
const SCROLL_SPEED: f64 = 1200.0; // wheel units/s (120 = one notch)
const SHIFT_BOOST: f64 = 3.0;
const SPACE_SLOW_MOVE: f64 = 0.20;
const SPACE_SLOW_SCROLL: f64 = 0.25;
/// Worker tick while moving or scrolling; it sleeps until woken otherwise.
const TICK: Duration = Duration::from_millis(2);

// ---- virtual keys ----
const LSHIFT: usize = 0xA0;
const RSHIFT: usize = 0xA1;
const LCTRL: usize = 0xA2;
const RCTRL: usize = 0xA3;
const LALT: usize = 0xA4;
const RALT: usize = 0xA5;
const CAPS: usize = 0x14;
const SPACE: usize = 0x20;
const KEY_I: usize = 0x49;
const KEY_J: usize = 0x4A;
const KEY_K: usize = 0x4B;
const KEY_L: usize = 0x4C;
const KEY_U: usize = 0x55;
const KEY_O: usize = 0x4F;
const KEY_H: usize = 0x48;
const KEY_Y: usize = 0x59;
const KEY_N: usize = 0x4E;
const KEY_M: usize = 0x4D;
const KEY_P: usize = 0x50;
const KEY_B: usize = 0x42;
const KEY_V: usize = 0x56;
const KEY_Q: usize = 0x51;
const COMMA: usize = 0xBC;
const PERIOD: usize = 0xBE;

const WM_REFRESH: u32 = WM_APP + 1;
const WM_MODE: u32 = WM_APP + 2;
const TIMER_BADGE: usize = 1;

// ---- shared state ----
const fn flags() -> [AtomicBool; 256] {
    [const { AtomicBool::new(false) }; 256]
}
/// Physical (non-injected) key state, tracked for every key.
static HELD: [AtomicBool; 256] = flags();
/// Keys whose down event we swallowed, so the matching up is swallowed too.
static SWALLOWED: [AtomicBool; 256] = flags();
/// Scroll keys armed in mouse mode: u, o, h, y.
static SCROLL: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];
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

fn tap_vk(vk: u16) {
    send(&[key_input(vk, 0, KEYBD_EVENT_FLAGS(0)), key_input(vk, 0, KEYEVENTF_KEYUP)]);
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
    jump_within(cursor_monitor(), JUMPS[index]);
}

fn jump_next_monitor() {
    let all = monitors();
    let cur = cursor_monitor();
    let i = all.iter().position(|m| m.0 == cur.0).unwrap_or(0);
    jump_within(all[(i + 1) % all.len()], [0.5, 0.5]);
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
        SWALLOWED[i].store(false, Relaxed);
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
    let lvl = (SPEED_LEVEL.load(Relaxed) as isize + delta).clamp(1, SPEEDS.len() as isize);
    SPEED_LEVEL.store(lvl as usize, Relaxed);
    post(WM_REFRESH);
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

    if up {
        match vk {
            KEY_U => SCROLL[0].store(false, Relaxed),
            KEY_O => SCROLL[1].store(false, Relaxed),
            KEY_H => SCROLL[2].store(false, Relaxed),
            KEY_Y => SCROLL[3].store(false, Relaxed),
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

    match vk {
        // Movement is driven by the worker from HELD; Space is the slow modifier.
        KEY_I | KEY_J | KEY_K | KEY_L | SPACE => {}
        KEY_N => click(MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        COMMA => click(MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        KEY_M => {
            if !repeat {
                toggle_drag();
            }
        }
        KEY_U | KEY_O | KEY_H | KEY_Y => scroll_key(vk),
        KEY_P => change_speed(1),
        PERIOD => change_speed(-1),
        // Back/Forward, except with Ctrl so Ctrl+V / Ctrl+B pass through.
        KEY_B | KEY_V => {
            if ctrl_phys() {
                return false;
            }
            tap_vk(if vk == KEY_B { VK_BROWSER_FORWARD.0 } else { VK_BROWSER_BACK.0 });
        }
        _ => match k.scanCode {
            // Physical top-row 1-4: jump near monitor corners.
            sc @ 0x02..=0x05 => {
                if !repeat {
                    jump_corner(sc as usize - 2);
                }
            }
            // Physical ` : center of the next monitor.
            0x29 => {
                if !repeat {
                    jump_next_monitor();
                }
            }
            // Physical ; : left click.
            0x27 => click(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            _ => return false,
        },
    }
    SWALLOWED[vk].store(true, Relaxed);
    true
}

fn scroll_key(vk: usize) {
    if ctrl_phys() {
        // One whole notch per press so Ctrl+wheel zooms in steps.
        match vk {
            KEY_U => wheel(false, 120),
            KEY_O => wheel(false, -120),
            KEY_Y => wheel(true, 120),
            _ => wheel(true, -120),
        }
        return;
    }
    let i = match vk {
        KEY_U => 0,
        KEY_O => 1,
        KEY_H => 2,
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
    sent_v: i32,
    sent_h: i32,
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
        // A quick tap still scrolls one full notch.
        if self.sent_v != 0 && self.sent_v.abs() < 120 {
            wheel(false, if self.sent_v > 0 { 120 } else { -120 } - self.sent_v);
        }
        if self.sent_h != 0 && self.sent_h.abs() < 120 {
            wheel(true, if self.sent_h > 0 { 120 } else { -120 } - self.sent_h);
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
        let space = held(SPACE);
        let boost = if shift_phys() { SHIFT_BOOST } else { 1.0 };
        let mut active = false;

        // Cursor movement.
        let dx = held(KEY_L) as i32 - held(KEY_J) as i32;
        let dy = held(KEY_K) as i32 - held(KEY_I) as i32;
        if dx != 0 || dy != 0 {
            active = true;
            if !self.moving {
                self.moving = true;
                self.last = now;
            }
            let dt = (now - self.last).as_secs_f64().clamp(0.001, 0.05);
            self.last = now;
            let vel = SPEEDS[SPEED_LEVEL.load(Relaxed) - 1] * if space { SPACE_SLOW_MOVE } else { 1.0 } * boost;
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
                self.sent_v = 0;
                self.sent_h = 0;
                SCROLLING.store(true, Relaxed);
                // Apps treat Shift+wheel as horizontal; hide Shift while scrolling.
                if shift_phys() {
                    SHIFT_HIDDEN.store(true, Relaxed);
                    lshift(true);
                }
            }
            let dt = (now - self.scroll_last).as_secs_f64().min(0.05);
            self.scroll_last = now;
            let vel = SCROLL_SPEED * if space { SPACE_SLOW_SCROLL } else { 1.0 } * boost;
            self.sfv += v as f64 * vel * dt;
            self.sfh += h as f64 * vel * dt;
            let dv = self.sfv.round();
            if dv != 0.0 {
                self.sfv -= dv;
                self.sent_v += dv as i32;
                wheel(false, dv as i32);
            }
            let dh = self.sfh.round();
            if dh != 0.0 {
                self.sfh -= dh;
                self.sent_h += dh as i32;
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
        sent_v: 0,
        sent_h: 0,
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

fn update_badge(hwnd: HWND) {
    unsafe {
        let mon = cursor_monitor();
        BADGE_MONITOR.store(mon.0 as isize, Relaxed);
        let r = work_area(mon);
        let (w, h) = (px(50), px(26));
        let (x, y) = (r.left + px(16), r.bottom - h - px(16));
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        for (i, v) in [x, y, w, h].into_iter().enumerate() {
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
    if cursor_monitor().0 as isize != BADGE_MONITOR.load(Relaxed) {
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
        WM_TIMER => {
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

        // Real Caps Lock starts off; Caps is swallowed from here on.
        if GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 {
            tap_vk(VK_CAPITAL.0);
        }

        let hwnd = create_badge();
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), Some(hinst), 0).unwrap();
        let t = std::thread::spawn(worker);
        let _ = WORKER.set(t.thread().clone());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        release_drag();
        let _ = UnhookWindowsHookEx(hook);
        let _ = WTSUnRegisterSessionNotification(hwnd);
        let _ = DestroyWindow(hwnd);
    }
}
