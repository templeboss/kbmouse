#Requires AutoHotkey v2.0
#SingleInstance Force

; Caps Lock toggles the mouse layer.
; Ctrl+Alt+Q exits the script entirely.
SetCapsLockState "AlwaysOff"
CoordMode "Mouse", "Screen"

mouseMode := false
speedLevel := 3
speeds := [800, 1600, 2400, 3000, 4000]
; Positions within the cursor's current monitor, 10% in from each edge.
jumpPositions := [[0.10, 0.10], [0.90, 0.10], [0.10, 0.90], [0.90, 0.90]]
moving := false
lastTick := A_TickCount
fractionX := 0.0
fractionY := 0.0
dragging := false

; Click-through, always on top, and does not take keyboard focus.
badge := Gui("+AlwaysOnTop -Caption +ToolWindow +E0x08000020", "Keyboard mouse")
badge.BackColor := "17212B"
badge.MarginX := 14
badge.MarginY := 10
badge.SetFont("s11 Bold", "Segoe UI")
badgeText := badge.AddText("c76E4AD w330", "MOUSE MODE  |  SPEED 3/5")
badgeBounds := 0
badgeVisible := false
OnExit Cleanup

CapsLock::ToggleMouseMode()
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
*SC027::Click "Left"
*n::Click "Right"
*,::Click "Middle"
*m::ToggleDrag()
*u::SendEvent "{WheelUp}"
*o::SendEvent "{WheelDown}"
*h::SendEvent "{WheelLeft}"
*y::SendEvent "{WheelRight}"
*b::SendEvent "{Browser_Forward}"
*v::SendEvent "{Browser_Back}"
*p::ChangeSpeed(1)
*.::ChangeSpeed(-1)
#HotIf

ToggleMouseMode() {
    global mouseMode, badge, badgeVisible
    mouseMode := !mouseMode
    StopMovement()
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
    global badge, badgeText, speedLevel, dragging, badgeBounds, badgeVisible
    badgeText.Text := "MOUSE MODE  |  SPEED " speedLevel "/5" (dragging ? "  |  DRAG" : "")
    MonitorGetWorkArea MonitorGetPrimary(), &left, &top, &right, &bottom
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
    global mouseMode, badge, badgeBounds, badgeVisible
    if !mouseMode || !IsObject(badgeBounds)
        return
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
    global jumpPositions, fractionX, fractionY, lastTick
    MouseGetPos &cursorX, &cursorY
    monitor := MonitorGetPrimary()
    Loop MonitorGetCount() {
        ; Full monitor bounds include the taskbar; targets use the work area.
        MonitorGet A_Index, &left, &top, &right, &bottom
        if (cursorX >= left && cursorX < right && cursorY >= top && cursorY < bottom) {
            monitor := A_Index
            break
        }
    }
    MonitorGetWorkArea monitor, &left, &top, &right, &bottom
    position := jumpPositions[index]
    x := left + Round((right - left - 1) * position[1])
    y := top + Round((bottom - top - 1) * position[2])
    fractionX := 0.0
    fractionY := 0.0
    lastTick := A_TickCount
    MouseMove x, y, 0
    ; One jump per press, even if the key is held.
    KeyWait Format("SC{:03X}", index + 1)
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

Cleanup(*) {
    SetTimer RefreshBadgeVisibility, 0
    StopMovement()
    ReleaseDrag()
}
