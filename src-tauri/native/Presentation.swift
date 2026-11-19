import AppKit
import ApplicationServices
import Foundation

private enum CaptureSignal {
    case active
    case clear
    case unknown
}

private let skippedRoles: Set<String> = [
    "AXWebArea", kAXTextAreaRole, kAXTextFieldRole, kAXListRole,
    kAXTableRole, kAXOutlineRole, kAXScrollAreaRole, kAXMenuRole, kAXMenuBarRole
]

private let browsers = [
    "com.apple.Safari", "com.google.Chrome", "com.microsoft.edgemac",
    "org.mozilla.firefox", "app.zen-browser.zen"
]

private func systemIndicator(_ label: String) -> Bool {
    [
        "screen sharing", "screen recording", "screen capture", "screenshare",
        "screen-share", "screen_share", "is sharing",
        "compartir pantalla", "compartiendo pantalla", "grabación de pantalla",
        "partage d’écran", "bildschirmfreigabe"
    ].contains { label.contains($0) }
}

private func browserIndicator(_ label: String) -> Bool {
    if ["stop sharing", "dejar de compartir", "detener el uso compartido"]
        .contains(label.trimmingCharacters(in: .punctuationCharacters)) {
        return true
    }
    return [
        "is sharing your screen", "is sharing a window", "is sharing a chrome tab",
        "you are sharing your entire screen", "you are sharing another application window",
        "compartiendo tu pantalla", "compartiendo una ventana", "compartiendo una pestaña",
        "choose what to share", "choose a screen to share", "choose a window to share",
        "select what to share", "select window or screen", "select a window or screen to share",
        "select a screen to share", "select a window to share", "share a window",
        "share this window", "share your screen", "share content", "elige qué compartir",
        "selecciona qué compartir", "selecciona una ventana", "selecciona una pantalla"
    ].contains { label.contains($0) }
}

private func attribute(_ element: AXUIElement, _ name: String) -> (AXError, CFTypeRef?) {
    var value: CFTypeRef?
    let result = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    return (result, value)
}

private func elements(_ value: CFTypeRef?) -> [AXUIElement] {
    guard let value else { return [] }
    if let list = value as? [AXUIElement] { return list }
    // AXExtrasMenuBar returns a single element. AXWindows and AXChildren return arrays.
    guard CFGetTypeID(value) == AXUIElementGetTypeID() else { return [] }
    return [unsafeBitCast(value, to: AXUIElement.self)]
}

private func signal(
    from bundleID: String, root: String, matches: (String) -> Bool
) -> CaptureSignal {
    let systemMenu = root == kAXExtrasMenuBarAttribute
    guard let application = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first else {
        return systemMenu ? .unknown : .clear
    }
    let element = AXUIElementCreateApplication(application.processIdentifier)
    AXUIElementSetMessagingTimeout(element, 0.03)
    let deadline = ProcessInfo.processInfo.systemUptime + 0.1
    let (result, value) = attribute(element, root)
    guard result == .success, value != nil else { return .unknown }
    let roots = elements(value)

    // Inspect native controls only. Page content and window titles are not sharing signals.
    var queue = roots.prefix(12).map { ($0, 0) }
    var index = 0
    var incomplete = roots.count > queue.count
    while index < queue.count {
        guard ProcessInfo.processInfo.systemUptime < deadline else { return .unknown }
        let (node, depth) = queue[index]
        index += 1
        let (roleResult, roleValue) = attribute(node, kAXRoleAttribute)
        incomplete = incomplete || roleResult == .cannotComplete
        let role = roleValue as? String ?? ""
        if skippedRoles.contains(role) && !(systemMenu && depth == 0) { continue }

        if role != kAXWindowRole {
            for name in [kAXTitleAttribute, kAXDescriptionAttribute, kAXIdentifierAttribute, kAXHelpAttribute, kAXValueAttribute] {
                if name == kAXIdentifierAttribute && !systemMenu { continue }
                if name == kAXValueAttribute &&
                    ![kAXStaticTextRole, kAXButtonRole, kAXMenuBarItemRole].contains(role) { continue }
                let (labelResult, labelValue) = attribute(node, name)
                incomplete = incomplete || labelResult == .cannotComplete
                if let label = labelValue as? String, label.count <= 240,
                   matches(label.lowercased().trimmingCharacters(in: .whitespacesAndNewlines)) {
                    return .active
                }
            }
        }

        if depth < (systemMenu ? 2 : 5) {
            let (childrenResult, childrenValue) = attribute(node, kAXChildrenAttribute)
            incomplete = incomplete || childrenResult == .cannotComplete
            let children = elements(childrenValue)
            let limit = min(systemMenu ? 24 : 16, 64 - queue.count)
            queue.append(contentsOf: children.prefix(limit).map { ($0, depth + 1) })
            incomplete = incomplete || children.count > limit
        }
    }
    return incomplete ? .unknown : .clear
}

func monitorPresentation() -> Never {
    let controlCenter = "com.apple.controlcenter"
    var activeSources: Set<String> = []
    var clearSince: TimeInterval?

    let timer = Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { _ in
        guard AXIsProcessTrusted() else {
            if !activeSources.isEmpty { respond("visible") }
            activeSources.removeAll()
            clearSince = nil
            return
        }

        var detected = false
        var observed: [String: CaptureSignal] = [:]
        func record(_ source: String, _ signal: CaptureSignal) {
            observed[source] = signal
            if signal == .active {
                if activeSources.isEmpty { respond("hidden") }
                activeSources.insert(source)
                detected = true
            }
        }
        record(controlCenter, signal(
            from: controlCenter, root: kAXExtrasMenuBarAttribute, matches: systemIndicator
        ))
        // Retain browser signals even when macOS also reports a capture so either can bridge a gap.
        for browser in browsers {
            record(browser, signal(from: browser, root: kAXWindowsAttribute, matches: browserIndicator))
        }
        if detected {
            clearSince = nil
        } else if !activeSources.isEmpty {
            if activeSources.contains(where: { observed[$0] == .unknown }) {
                clearSince = nil
                return
            }
            let now = ProcessInfo.processInfo.systemUptime
            if let since = clearSince, now - since >= 2 {
                activeSources.removeAll()
                clearSince = nil
                respond("visible")
            } else if clearSince == nil {
                clearSince = now
            }
        }
    }

    // Match the Activity worker lifecycle. The monitor never posts input or holds a sleep assertion.
    DispatchQueue.global().async {
        _ = FileHandle.standardInput.readDataToEndOfFile()
        exit(EXIT_SUCCESS)
    }
    timer.fire()
    RunLoop.main.run()
    exit(EXIT_SUCCESS)
}
