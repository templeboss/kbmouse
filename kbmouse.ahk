#Requires AutoHotkey v2.0
#SingleInstance Force

; Relaunch elevated so it works over admin windows (installers, games, elevated tray menus).
if !A_IsAdmin {
    try Run '*RunAs "' A_AhkPath '" "' A_ScriptFullPath '"'
    ExitApp
}

; Caps Lock toggles the mouse layer.
; Ctrl+Alt+Q exits the script entirely.
SetCapsLockState "AlwaysOff"
CoordMode "Mouse", "Screen"

mouseMode := false
speedLevel := 3
speeds := [1800, 2400, 3000, 4000, 5200]
; Positions within the cursor's current monitor, 10% in from each edge.
jumpPositions := [[0.10, 0.10], [0.90, 0.10], [0.10, 0.90], [0.90, 0.90]]
moving := false
lastTick := A_TickCount
fractionX := 0.0
fractionY := 0.0
dragging := false
; Smooth scroll speed in wheel units per second (120 = one notch).
scrollSpeed := 1200
scrolling := false
scrollTick := 0
scrollFractionV := 0.0
scrollFractionH := 0.0
scrollSentV := 0
scrollSentH := 0

; Click-through, always on top, and does not take keyboard focus.
badge := Gui("+AlwaysOnTop -Caption +ToolWindow +E0x08000020", "Keyboard mouse")
badge.BackColor := "17212B"
badge.MarginX := 8
badge.MarginY := 4
; Mouse glyph; turns amber while dragging.
badge.SetFont("s10", "Segoe Fluent Icons")
badgeIcon := badge.AddText("c76E4AD", Chr(0xE962))
; Current speed level.
badge.SetFont("s10 Bold", "Segoe UI")
badgeText := badge.AddText("x+5 yp-2 c76E4AD", "3")
WinSetTransparent 190, badge.Hwnd
; Windows 11 rounded corners (DWMWA_WINDOW_CORNER_PREFERENCE = round).
DllCall("dwmapi\DwmSetWindowAttribute", "Ptr", badge.Hwnd, "UInt", 33, "Int*", 2, "UInt", 4)
badgeBounds := 0
badgeVisible := false
badgeMonitor := 0
OnExit Cleanup

CapsLock::ToggleMouseMode()
; Escape hatch: Shift+CapsLock turns mouse mode off and toggles the real Caps Lock.
+CapsLock::RealCapsLock()
^!q::ExitApp()

#HotIf mouseMode
*i::StartMovement()
*j::StartMovement()
*k::StartMovement()
*l::StartMovement()
*Space::return
; Physical top-row 1-4: jump near the current monitor's corners.
*SC002::JumpToPosition(1)
*SC003::JumpToPosition(2)
*SC004::JumpToPosition(3)
*SC005::JumpToPosition(4)
; Physical ` key: cycle to the center of the next monitor.
*SC029::JumpToNextMonitor()
*SC027::Click "Left"
*n::Click "Right"
*,::Click "Middle"
*m::ToggleDrag()
*u::StartScroll("{WheelUp}")
*o::StartScroll("{WheelDown}")
*h::StartScroll("{WheelLeft}")
*y::StartScroll("{WheelRight}")
*p::ChangeSpeed(1)
*.::ChangeSpeed(-1)
#HotIf
; v = Back, b = Forward, except with Ctrl held so Ctrl+V / Ctrl+(Shift+)B work normally.
#HotIf mouseMode && !GetKeyState("Ctrl", "P")
*b::SendEvent "{Browser_Forward}"
*v::SendEvent "{Browser_Back}"
#HotIf

RealCapsLock() {
    global mouseMode
    if mouseMode
        ToggleMouseMode()
    ; Real Caps Lock on → back to the forced-off state; off → turn it on.
    SetCapsLockState GetKeyState("CapsLock", "T") ? "AlwaysOff" : "On"
}

ToggleMouseMode() {
    global mouseMode, badge, badgeVisible
    mouseMode := !mouseMode
    StopMovement()
    StopScroll()
    if mouseMode {
        UpdateBadge()
        SetTimer RefreshBadgeVisibility, 50
        if MovementKeyDown()
            StartMovement()
    } else {
        SetTimer RefreshBadgeVisibility, 0
        ReleaseDrag()
        badge.Hide()
        badgeVisible := false
    }
}

UpdateBadge() {
    global badge, badgeIcon, badgeText, speedLevel, dragging, badgeBounds, badgeVisible, badgeMonitor
    badgeIcon.SetFont(dragging ? "cF2A33A" : "c76E4AD")
    badgeText.Text := speedLevel
    ; Badge follows the monitor the cursor is on.
    badgeMonitor := CursorMonitor()
    MonitorGetWorkArea badgeMonitor, &left, &top, &right, &bottom
    badge.Show("Hide AutoSize")
    badge.GetPos(, , &width, &height)
    badge.Show("Hide x" (left + 16) " y" (bottom - height - 16))
    badge.GetPos(&x, &y, &width, &height)
    ; Keep the bounds even while hidden so moving away restores the badge.
    badgeBounds := {x: x, y: y, width: width, height: height}
    badgeVisible := false
    RefreshBadgeVisibility()
}

RefreshBadgeVisibility() {
    global mouseMode, badge, badgeBounds, badgeVisible, badgeMonitor
    if !mouseMode || !IsObject(badgeBounds)
        return
    if (CursorMonitor() != badgeMonitor) {
        UpdateBadge()
        return
    }
    MouseGetPos &x, &y
    overBadge := x >= badgeBounds.x && x < badgeBounds.x + badgeBounds.width
        && y >= badgeBounds.y && y < badgeBounds.y + badgeBounds.height
    shouldShow := !overBadge
    if (shouldShow = badgeVisible)
        return
    if shouldShow
        badge.Show("NoActivate")
    else
        badge.Hide()
    badgeVisible := shouldShow
}

ChangeSpeed(delta) {
    global speedLevel, speeds
    speedLevel := Max(1, Min(speeds.Length, speedLevel + delta))
    UpdateBadge()
}

ToggleDrag() {
    global dragging
    dragging := !dragging
    SendEvent dragging ? "{LButton Down}" : "{LButton Up}"
    UpdateBadge()
    ; One toggle per press; autorepeat would flip the button while M is held.
    KeyWait "m"
}

ReleaseDrag() {
    global dragging
    if dragging {
        SendEvent "{LButton Up}"
        dragging := false
    }
}

MovementKeyDown() {
    return GetKeyState("i", "P") || GetKeyState("j", "P")
        || GetKeyState("k", "P") || GetKeyState("l", "P")
}

JumpToPosition(index) {
    global jumpPositions
    JumpWithinMonitor(CursorMonitor(), jumpPositions[index])
    ; One jump per press, even if the key is held.
    KeyWait Format("SC{:03X}", index + 1)
}

JumpToNextMonitor() {
    JumpWithinMonitor(Mod(CursorMonitor(), MonitorGetCount()) + 1, [0.5, 0.5])
    KeyWait "SC029"
}

; Monitor that contains the cursor (falls back to the primary).
CursorMonitor() {
    MouseGetPos &cursorX, &cursorY
    Loop MonitorGetCount() {
        ; Full monitor bounds include the taskbar; targets use the work area.
        MonitorGet A_Index, &left, &top, &right, &bottom
        if (cursorX >= left && cursorX < right && cursorY >= top && cursorY < bottom)
            return A_Index
    }
    return MonitorGetPrimary()
}

; Moves to [x, y] fractions of a monitor's work area.
JumpWithinMonitor(monitor, position) {
    global fractionX, fractionY, lastTick
    MonitorGetWorkArea monitor, &left, &top, &right, &bottom
    x := left + Round((right - left - 1) * position[1])
    y := top + Round((bottom - top - 1) * position[2])
    fractionX := 0.0
    fractionY := 0.0
    lastTick := A_TickCount
    MouseMove x, y, 0
}

StartMovement() {
    global moving, lastTick, mouseMode
    if moving || !mouseMode
        return
    moving := true
    lastTick := A_TickCount
    SetTimer MovePointer, 10
}

StopMovement() {
    global moving, fractionX, fractionY
    SetTimer MovePointer, 0
    moving := false
    fractionX := 0.0
    fractionY := 0.0
}

MovePointer() {
    global lastTick, fractionX, fractionY, speedLevel, speeds, mouseMode
    if !mouseMode || !MovementKeyDown() {
        StopMovement()
        return
    }
    now := A_TickCount
    dt := Min(0.05, Max(0.001, (now - lastTick) / 1000.0))
    lastTick := now

    dx := (GetKeyState("l", "P") ? 1 : 0) - (GetKeyState("j", "P") ? 1 : 0)
    dy := (GetKeyState("k", "P") ? 1 : 0) - (GetKeyState("i", "P") ? 1 : 0)
    if (dx = 0 && dy = 0) {

        fractionX := 0.0
        fractionY := 0.0
        return
    }

    velocity := speeds[speedLevel] * (GetKeyState("Space", "P") ? 0.20 : 1.0)
        * (GetKeyState("Shift", "P") ? 1.5 : 1.0)

    ; Normalize diagonals so they aren't faster.
    if (dx != 0 && dy != 0) {
        dx *= 0.70710678
        dy *= 0.70710678
    }

    fractionX += dx * velocity * dt
    fractionY += dy * velocity * dt
    moveX := Round(fractionX)
    moveY := Round(fractionY)
    fractionX -= moveX
    fractionY -= moveY
    if (moveX != 0 || moveY != 0)
        MouseMove moveX, moveY, 0, "R"
}

StartScroll(notch) {
    global scrolling, scrollTick, scrollFractionV, scrollFractionH, scrollSentV, scrollSentH
    ; With Ctrl held, send whole notches (one per press/autorepeat) so zoom
    ; steps normally. {Blind} keeps Ctrl down so the app sees Ctrl+wheel.
    if GetKeyState("Ctrl", "P") {
        SendEvent "{Blind}" notch
        return
    }
    ; Ignore keyboard autorepeat; the timer drives scrolling while held.
    if scrolling
        return
    scrolling := true
    scrollTick := A_TickCount
    scrollFractionV := 0.0
    scrollFractionH := 0.0
    scrollSentV := 0
    scrollSentH := 0
    ScrollStep()
    SetTimer ScrollStep, 10
}

ScrollStep() {
    global mouseMode, scrollSpeed, scrollTick, scrollFractionV, scrollFractionH, scrollSentV, scrollSentH
    v := (GetKeyState("u", "P") ? 1 : 0) - (GetKeyState("o", "P") ? 1 : 0)
    h := (GetKeyState("y", "P") ? 1 : 0) - (GetKeyState("h", "P") ? 1 : 0)
    if !mouseMode || (v = 0 && h = 0) || GetKeyState("Ctrl", "P") {
        StopScroll()
        return
    }
    ; Apps treat Shift+wheel as horizontal scroll, so keep Shift logically up.
    if GetKeyState("Shift")
        SendEvent "{Blind}{Shift up}"
    now := A_TickCount
    dt := Min(0.05, (now - scrollTick) / 1000.0)
    scrollTick := now
    velocity := scrollSpeed * (GetKeyState("Space", "P") ? 0.25 : 1.0)
        * (GetKeyState("Shift", "P") ? 2.0 : 1.0)
    scrollFractionV += v * velocity * dt
    scrollFractionH += h * velocity * dt
    scrollSentV += SendWheel(0x0800, &scrollFractionV)
    scrollSentH += SendWheel(0x1000, &scrollFractionH)
}

StopScroll() {
    global scrolling, scrollSentV, scrollSentH
    if !scrolling
        return
    SetTimer ScrollStep, 0
    scrolling := false
    ; Restore Shift if it is still physically held.
    if GetKeyState("Shift", "P") && !GetKeyState("Shift")
        SendEvent "{Blind}{Shift down}"
    ; A quick tap still scrolls one full notch.
    if (Abs(scrollSentV) > 0 && Abs(scrollSentV) < 120)
        WheelEvent(0x0800, (scrollSentV > 0 ? 120 : -120) - scrollSentV)
    if (Abs(scrollSentH) > 0 && Abs(scrollSentH) < 120)
        WheelEvent(0x1000, (scrollSentH > 0 ? 120 : -120) - scrollSentH)
}

; Sends the whole-unit part of an accumulated wheel delta; returns what was sent.
SendWheel(flag, &fraction) {
    delta := Round(fraction)
    if (delta = 0)
        return 0
    fraction -= delta
    WheelEvent(flag, delta)
    return delta
}

; 0x0800 = vertical wheel (positive up), 0x1000 = horizontal (positive right).
WheelEvent(flag, delta) {
    DllCall("mouse_event", "UInt", flag, "Int", 0, "Int", 0, "Int", delta, "UPtr", 0)
}

Cleanup(*) {
    ; Hand Caps Lock control back to Windows when the script exits.
    SetCapsLockState "Off"
    SetTimer RefreshBadgeVisibility, 0
    StopMovement()
    StopScroll()
    ReleaseDrag()
}
