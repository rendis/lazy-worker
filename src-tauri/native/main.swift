import ApplicationServices
import Foundation
import IOKit
import IOKit.pwr_mgt

func respond(_ message: String) {
    print(message)
    fflush(stdout)
}

switch CommandLine.arguments.dropFirst().first {
case "--accessibility-status":
    respond(AXIsProcessTrusted() ? "granted" : "required")
    exit(EXIT_SUCCESS)
case "--app-icon":
    exit(writeAppIcon(CommandLine.arguments.dropFirst(2).first))
case "--monitor-presentation":
    monitorPresentation()
default:
    break
}

let hidService = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOHIDSystem"))

func idleSeconds() -> TimeInterval? {
    guard hidService != 0,
          let value = IORegistryEntryCreateCFProperty(
              hidService, "HIDIdleTime" as CFString, kCFAllocatorDefault, 0
          )?.takeRetainedValue() as? NSNumber
    else { return nil }
    return value.doubleValue / 1_000_000_000
}

func pointerButtonIsDown() -> Bool {
    [.left, .right, .center].contains {
        CGEventSource.buttonState(.hidSystemState, button: $0)
    }
}

var gate = IdleGate()

func movePointerIfIdle() {
    let now = ProcessInfo.processInfo.systemUptime
    guard gate.allowsMovement(idleSeconds: idleSeconds(), at: now),
          !pointerButtonIsDown() else { return }

    guard let cursor = CGEvent(source: nil)?.location else { return }

    var display: CGDirectDisplayID = 0
    var displayCount: UInt32 = 0
    guard CGGetDisplaysWithPoint(cursor, 1, &display, &displayCount) == .success,
          displayCount > 0 else { return }

    let edge = CGDisplayBounds(display).maxX
    let offset: CGFloat = cursor.x + 2 < edge ? 2 : -2
    let nearby = CGPoint(x: cursor.x + offset, y: cursor.y)
    guard let move = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved,
                             mouseCursorPosition: nearby, mouseButton: .left),
          let restore = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved,
                                mouseCursorPosition: cursor, mouseButton: .left)
    else { return }

    guard gate.allowsMovement(idleSeconds: idleSeconds(), at: now),
          !pointerButtonIsDown(),
          CGEvent(source: nil)?.location == cursor else { return }

    move.post(tap: .cghidEventTap)
    restore.post(tap: .cghidEventTap)
    gate.recordMovement(at: now)
}

guard AXIsProcessTrusted() else {
    respond("permissionRequired")
    exit(EXIT_FAILURE)
}

guard idleSeconds() != nil else {
    respond("idleMonitorUnavailable")
    exit(EXIT_FAILURE)
}

var assertion = IOPMAssertionID(0)
let result = IOPMAssertionCreateWithName(
    kIOPMAssertionTypePreventUserIdleSystemSleep as CFString,
    IOPMAssertionLevel(kIOPMAssertionLevelOn),
    "Lazy Worker activity session" as CFString,
    &assertion
)
guard result == kIOReturnSuccess else {
    respond("workerUnavailable")
    exit(EXIT_FAILURE)
}

respond("ready")
movePointerIfIdle()
let timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
    movePointerIfIdle()
}

// The parent owns the write end of stdin. Closing it ends the helper when the app quits.
DispatchQueue.global().async {
    _ = FileHandle.standardInput.readDataToEndOfFile()
    DispatchQueue.main.async {
        timer.invalidate()
        IOPMAssertionRelease(assertion)
        IOObjectRelease(hidService)
        exit(EXIT_SUCCESS)
    }
}

RunLoop.main.run()
