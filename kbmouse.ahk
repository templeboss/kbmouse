#Requires AutoHotkey v2.0
#SingleInstance Force

; Caps Lock toggles the mouse layer.
; Escape exits it. Ctrl+Alt+Q exits the script entirely.
SetCapsLockState "AlwaysOff"

mouseMode := false
speedLevel := 3
movementTime := 0
lastTick := A_TickCount
fractionX := 0.0
fractionY := 0.0
dragging := false

CapsLock::ToggleMouseMode()
^!q::ExitApp()

#HotIf mouseMode
*i::return
*j::return
*k::return
*l::return
*Space::return
*SC027::Click "Left"
*n::Click "Right"
*,::Click "Middle"
*m::ToggleDrag()
*u::SendEvent "{WheelUp}"
*o::SendEvent "{WheelDown}"
*p::ChangeSpeed(1)
*.::ChangeSpeed(-1)
#HotIf

ToggleMouseMode() {
    global mouseMode, movementTime, lastTick, fractionX, fractionY, dragging
    mouseMode := !mouseMode
    movementTime := 0
    lastTick := A_TickCount
    fractionX := 0.0
    fractionY := 0.0
    if !mouseMode && dragging {
        SendEvent "{LButton Up}"
        dragging := false
    }
    SetTimer MovePointer, mouseMode ? 10 : 0
    ToolTip mouseMode ? "MOUSE MODE ON" : "MOUSE MODE OFF"
    SetTimer HideTip, -700
}

HideTip() => ToolTip()

ChangeSpeed(delta) {
    global speedLevel
    speedLevel := Max(1, Min(5, speedLevel + delta))
    ToolTip "Mouse speed: " speedLevel "/5"
    SetTimer HideTip, -700
}

ToggleDrag() {
    global dragging
    dragging := !dragging
    SendEvent dragging ? "{LButton Down}" : "{LButton Up}"
    ToolTip dragging ? "DRAG ON" : "DRAG OFF"
    SetTimer HideTip, -700
}

MovePointer() {
    global movementTime, lastTick, fractionX, fractionY, speedLevel
    now := A_TickCount
    dt := Min(0.05, Max(0.001, (now - lastTick) / 1000.0))
    lastTick := now

    dx := (GetKeyState("l", "P") ? 1 : 0) - (GetKeyState("j", "P") ? 1 : 0)
    dy := (GetKeyState("k", "P") ? 1 : 0) - (GetKeyState("i", "P") ? 1 : 0)
    if (dx = 0 && dy = 0) {

        movementTime := 0
        fractionX := 0.0
        fractionY := 0.0
        return
    }

    ; Linear acceleration over the first 0.6 seconds.
    ; Speed presets, in pixels per second.
    speeds := [800, 1600, 2400, 3000, 4000]
    maxSpeed := speeds[speedLevel]
    velocity := maxSpeed 
    if GetKeyState("Space", "P")
        velocity *= 0.20

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
