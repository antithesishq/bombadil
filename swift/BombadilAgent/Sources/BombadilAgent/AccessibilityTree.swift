#if os(macOS)

import AppKit

/// Serializes the application's accessibility hierarchy into the wire
/// format. Must be called on the main thread.
enum AccessibilityTree {

    /// Height of the primary screen, used to flip between Cocoa's
    /// bottom-left origin and the protocol's top-left origin.
    private static var screenHeight: CGFloat {
        NSScreen.screens.first?.frame.height ?? 0
    }

    /// Convert a Cocoa screen rect (bottom-left origin) to the wire
    /// frame (top-left origin).
    private static func toWireFrame(
        _ rect: NSRect, screenHeight: CGFloat
    ) -> Wire.Frame {
        Wire.Frame(
            x: rect.origin.x,
            y: screenHeight - (rect.origin.y + rect.height),
            width: rect.width,
            height: rect.height
        )
    }

    /// Convert a wire point (top-left origin) to a Cocoa screen point.
    static func toCocoaPoint(x: Double, y: Double) -> NSPoint {
        NSPoint(x: x, y: screenHeight - y)
    }

    static func snapshot() -> Wire.Node {
        precondition(Thread.isMainThread)
        // Queried once per snapshot; `toWireFrame` runs for every node.
        let screenHeight = self.screenHeight
        let windows = NSApp.windows
            .filter { $0.isVisible }
            .map { node(for: $0, screenHeight: screenHeight) }
        let applicationFrame =
            NSScreen.screens.first.map {
                toWireFrame($0.frame, screenHeight: screenHeight)
            }
            ?? Wire.Frame(x: 0, y: 0, width: 0, height: 0)
        return Wire.Node(
            role: "Application",
            identifier: nil,
            label: ProcessInfo.processInfo.processName,
            value: nil,
            frame: applicationFrame,
            enabled: true,
            selected: false,
            focused: NSApp.isActive,
            children: windows
        )
    }

    private static func node(
        for window: NSWindow, screenHeight: CGFloat
    ) -> Wire.Node {
        Wire.Node(
            role: "Window",
            identifier: window.identifier?.rawValue,
            label: window.title,
            value: nil,
            frame: toWireFrame(window.frame, screenHeight: screenHeight),
            enabled: true,
            selected: window.isKeyWindow,
            focused: window.isKeyWindow,
            children: children(of: window, screenHeight: screenHeight)
        )
    }

    private static func children(
        of element: Any, screenHeight: CGFloat
    ) -> [Wire.Node] {
        guard let element = element as? NSAccessibilityProtocol else {
            return []
        }
        var rawChildren: [Any] = element.accessibilityChildren() ?? []
        if rawChildren.isEmpty,
            let ordered = element.accessibilityChildrenInNavigationOrder()
        {
            rawChildren = ordered.map { $0 as Any }
        }
        return rawChildren.compactMap {
            node(forElement: $0, screenHeight: screenHeight)
        }
    }

    private static func node(
        forElement element: Any, screenHeight: CGFloat
    ) -> Wire.Node? {
        guard let accessible = element as? NSAccessibilityProtocol else {
            return nil
        }

        let role = accessible.accessibilityRole()?.rawValue ?? "Unknown"
        let frame = accessible.accessibilityFrame()
        let childNodes = children(of: element, screenHeight: screenHeight)

        // Skip structural noise: unlabelled zero-size groups without
        // children carry no information for the fuzzer.
        if frame.width == 0, frame.height == 0, childNodes.isEmpty {
            return nil
        }

        return Wire.Node(
            role: normalize(role: role),
            identifier: nonEmpty(accessible.accessibilityIdentifier()),
            label: nonEmpty(accessible.accessibilityLabel()),
            value: valueDescription(accessible.accessibilityValue()),
            frame: toWireFrame(frame, screenHeight: screenHeight),
            enabled: accessible.isAccessibilityEnabled(),
            selected: accessible.isAccessibilitySelected(),
            focused: accessible.isAccessibilityFocused(),
            children: childNodes
        )
    }

    /// "AXButton" → "Button", matching the roles the default
    /// specification knows about.
    private static func normalize(role: String) -> String {
        if role.hasPrefix("AX") {
            return String(role.dropFirst(2))
        }
        return role
    }

    private static func nonEmpty(_ value: String?) -> String? {
        guard let value = value, !value.isEmpty else {
            return nil
        }
        return value
    }

    private static func valueDescription(_ value: Any?) -> String? {
        switch value {
        case nil: return nil
        case let string as String: return string
        case let number as NSNumber: return number.stringValue
        case let other?: return String(describing: other)
        }
    }
}

#endif
