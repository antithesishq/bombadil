#if os(macOS)

import AppKit

/// Applies driver actions by synthesizing in-process events. Since the
/// events are delivered directly to this app's windows, no system
/// accessibility permissions are required. Must be called on the main
/// thread.
enum ActionPerformer {

    static func perform(_ action: Wire.Action) throws {
        precondition(Thread.isMainThread)
        switch action {
        case .tap(let x, let y):
            try tap(x: x, y: y)
        case .typeText(let text):
            try type(text: text)
        case .pressKey(let key):
            try press(key: key)
        case .scrollUp(let x, let y, let distance):
            try scroll(x: x, y: y, deltaY: distance)
        case .scrollDown(let x, let y, let distance):
            try scroll(x: x, y: y, deltaY: -distance)
        }
    }

    private static func window(
        atScreenPoint point: NSPoint
    ) -> NSWindow? {
        // Frontmost windows first, so overlapping windows resolve the
        // way a real click would.
        for window in NSApp.orderedWindows where window.isVisible {
            if window.frame.contains(point) {
                return window
            }
        }
        return nil
    }

    private static func tap(x: Double, y: Double) throws {
        let screenPoint = AccessibilityTree.toCocoaPoint(x: x, y: y)
        guard let window = window(atScreenPoint: screenPoint) else {
            throw AgentError.actionFailed(
                "no window at (\(x), \(y))")
        }
        let localPoint = window.convertPoint(fromScreen: screenPoint)

        for (kind, clickCount) in [
            (NSEvent.EventType.leftMouseDown, 1),
            (NSEvent.EventType.leftMouseUp, 1),
        ] {
            guard
                let event = NSEvent.mouseEvent(
                    with: kind,
                    location: localPoint,
                    modifierFlags: [],
                    timestamp: ProcessInfo.processInfo.systemUptime,
                    windowNumber: window.windowNumber,
                    context: nil,
                    eventNumber: 0,
                    clickCount: clickCount,
                    pressure: kind == .leftMouseDown ? 1 : 0
                )
            else {
                throw AgentError.actionFailed("could not create mouse event")
            }
            window.sendEvent(event)
        }
    }

    private static func type(text: String) throws {
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else {
            throw AgentError.actionFailed("no key window to type into")
        }
        guard let responder = window.firstResponder else {
            throw AgentError.actionFailed("no first responder to type into")
        }
        responder.insertText(text)
    }

    private static let keyCodes: [String: UInt16] = [
        "return": 36,
        "tab": 48,
        "space": 49,
        "delete": 51,
        "escape": 53,
        "left": 123,
        "right": 124,
        "down": 125,
        "up": 126,
    ]

    private static let keyCharacters: [String: String] = [
        "return": "\r",
        "tab": "\t",
        "space": " ",
        "delete": "\u{7f}",
        "escape": "\u{1b}",
        "left": "\u{f702}",
        "right": "\u{f703}",
        "down": "\u{f701}",
        "up": "\u{f700}",
    ]

    private static func press(key: String) throws {
        guard let keyCode = keyCodes[key],
            let characters = keyCharacters[key]
        else {
            throw AgentError.actionFailed("unknown key: \(key)")
        }
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow else {
            throw AgentError.actionFailed("no key window to send keys to")
        }
        for kind in [NSEvent.EventType.keyDown, NSEvent.EventType.keyUp] {
            guard
                let event = NSEvent.keyEvent(
                    with: kind,
                    location: .zero,
                    modifierFlags: [],
                    timestamp: ProcessInfo.processInfo.systemUptime,
                    windowNumber: window.windowNumber,
                    context: nil,
                    characters: characters,
                    charactersIgnoringModifiers: characters,
                    isARepeat: false,
                    keyCode: keyCode
                )
            else {
                throw AgentError.actionFailed("could not create key event")
            }
            window.sendEvent(event)
        }
    }

    private static func scroll(
        x: Double, y: Double, deltaY: Double
    ) throws {
        let screenPoint = AccessibilityTree.toCocoaPoint(x: x, y: y)
        guard let window = window(atScreenPoint: screenPoint) else {
            throw AgentError.actionFailed("no window at (\(x), \(y))")
        }
        guard
            let cgEvent = CGEvent(
                scrollWheelEvent2Source: nil,
                units: .pixel,
                wheelCount: 1,
                wheel1: Int32(deltaY),
                wheel2: 0,
                wheel3: 0
            )
        else {
            throw AgentError.actionFailed("could not create scroll event")
        }
        // CGEvent locations use top-left-origin global coordinates,
        // which is exactly the wire coordinate space.
        cgEvent.location = CGPoint(x: x, y: y)
        guard let event = NSEvent(cgEvent: cgEvent) else {
            throw AgentError.actionFailed("could not wrap scroll event")
        }
        window.sendEvent(event)
    }
}

#endif
