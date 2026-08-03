#if os(macOS)

import AppKit

/// Serializes the application's accessibility hierarchy into the wire
/// format. Must be called on the main thread.
@MainActor
enum AccessibilityTree {

    // Keep even a worst-case JSON-escaped tree below LineConnection's
    // 16 MiB message ceiling.
    private static let maximumDepth = 64
    private static let maximumNodes = 2_048
    private static let maximumRoleBytes = 64
    private static let maximumStringBytes = 256

    @MainActor
    private struct Traversal {
        var seen: Set<ObjectIdentifier> = []
        var remainingNodes = AccessibilityTree.maximumNodes

        var hasCapacity: Bool {
            remainingNodes > 0
        }

        mutating func claim(_ object: NSObject, depth: Int) -> Bool {
            guard depth < AccessibilityTree.maximumDepth,
                remainingNodes > 0,
                seen.insert(ObjectIdentifier(object)).inserted
            else {
                return false
            }
            remainingNodes -= 1
            return true
        }
    }

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
        let width = dimension(rect.width)
        let height = dimension(rect.height)
        return Wire.Frame(
            x: finite(rect.origin.x),
            y: finite(screenHeight - (rect.origin.y + height)),
            width: width,
            height: height
        )
    }

    /// Convert a wire point (top-left origin) to a Cocoa screen point.
    static func toCocoaPoint(x: Double, y: Double) -> NSPoint {
        NSPoint(x: x, y: screenHeight - y)
    }

    static func snapshot() -> Wire.Node {
        // Queried once per snapshot; `toWireFrame` runs for every node.
        let screenHeight = self.screenHeight
        var traversal = Traversal()
        var windows: [Wire.Node] = []
        for window in NSApp.windows where window.isVisible {
            guard traversal.hasCapacity else {
                break
            }
            if let node = node(
                for: window,
                screenHeight: screenHeight,
                traversal: &traversal
            ) {
                windows.append(node)
            }
        }
        let applicationFrame =
            NSScreen.screens.first.map {
                toWireFrame($0.frame, screenHeight: screenHeight)
            }
            ?? Wire.Frame(x: 0, y: 0, width: 0, height: 0)
        return Wire.Node(
            role: "Application",
            identifier: nil,
            label: nonEmpty(ProcessInfo.processInfo.processName),
            value: nil,
            frame: applicationFrame,
            enabled: true,
            selected: false,
            focused: NSApp.isActive,
            children: windows
        )
    }

    private static func node(
        for window: NSWindow,
        screenHeight: CGFloat,
        traversal: inout Traversal
    ) -> Wire.Node? {
        guard traversal.claim(window, depth: 0) else {
            return nil
        }
        return Wire.Node(
            role: "Window",
            identifier: nonEmpty(window.identifier?.rawValue),
            label: nonEmpty(window.title),
            value: nil,
            frame: toWireFrame(window.frame, screenHeight: screenHeight),
            enabled: true,
            selected: window.isKeyWindow,
            focused: window.isKeyWindow,
            children: children(
                of: window,
                screenHeight: screenHeight,
                depth: 1,
                traversal: &traversal)
        )
    }

    /// Accessibility getters are queried via KVC rather than a cast
    /// to `NSAccessibilityProtocol`: SwiftUI's `AccessibilityNode`
    /// bridge objects implement the getter selectors without formally
    /// conforming to the protocol, so a conformance cast loses the
    /// whole SwiftUI subtree.
    private static func axValue(
        _ object: NSObject, _ key: String
    ) -> Any? {
        let capitalized = key.prefix(1).uppercased() + key.dropFirst()
        guard
            object.responds(to: NSSelectorFromString(key))
                || object.responds(
                    to: NSSelectorFromString("is\(capitalized)"))
        else {
            return nil
        }
        return object.value(forKey: key)
    }

    private static func children(
        of element: Any,
        screenHeight: CGFloat,
        depth: Int,
        traversal: inout Traversal
    ) -> [Wire.Node] {
        guard depth < maximumDepth, traversal.hasCapacity,
            let object = element as? NSObject
        else {
            return []
        }
        var rawChildren =
            axValue(object, "accessibilityChildren") as? [Any] ?? []
        if rawChildren.isEmpty,
            let ordered = axValue(
                object, "accessibilityChildrenInNavigationOrder")
                as? [Any]
        {
            rawChildren = ordered
        }
        var nodes: [Wire.Node] = []
        for child in rawChildren {
            guard traversal.hasCapacity else {
                break
            }
            if let node = node(
                forElement: child,
                screenHeight: screenHeight,
                depth: depth,
                traversal: &traversal
            ) {
                nodes.append(node)
            }
        }
        return nodes
    }

    /// Titlebar window controls. Hidden from the tree: they are
    /// indistinguishable from app buttons on the wire, and tapping
    /// close/minimize takes the whole window out of the test.
    private static let windowChromeSubroles: Set<NSAccessibility.Subrole> = [
        .closeButton, .minimizeButton, .zoomButton, .fullScreenButton,
    ]

    private static func node(
        forElement element: Any,
        screenHeight: CGFloat,
        depth: Int,
        traversal: inout Traversal
    ) -> Wire.Node? {
        guard let object = element as? NSObject else {
            return nil
        }

        let subrole =
            axValue(object, "accessibilitySubrole") as? String
        if let subrole,
            windowChromeSubroles.contains(
                NSAccessibility.Subrole(rawValue: subrole))
        {
            return nil
        }
        guard traversal.claim(object, depth: depth) else {
            return nil
        }

        let rawRole = bounded(
            axValue(object, "accessibilityRole") as? String
                ?? "Unknown",
            maximumBytes: maximumRoleBytes + 2)
        let role = bounded(
            normalize(role: rawRole),
            maximumBytes: maximumRoleBytes)
        let frame =
            (axValue(object, "accessibilityFrame") as? NSValue)?
            .rectValue ?? .zero
        let wireFrame = toWireFrame(frame, screenHeight: screenHeight)
        let identifier = nonEmpty(
            axValue(object, "accessibilityIdentifier") as? String)
        let label = nonEmpty(
            axValue(object, "accessibilityLabel") as? String)
        let isSecureTextField =
            subrole
            == NSAccessibility.Subrole.secureTextField.rawValue
        let value =
            isSecureTextField
            ? nil
            : valueDescription(axValue(object, "accessibilityValue"))
        let childNodes = children(
            of: element,
            screenHeight: screenHeight,
            depth: depth + 1,
            traversal: &traversal)

        // Skip structural noise: unlabelled zero-size groups without
        // children carry no information for the fuzzer.
        let hasSemanticValue =
            identifier != nil || label != nil || value != nil
        if wireFrame.width == 0 || wireFrame.height == 0,
            childNodes.isEmpty, !hasSemanticValue
        {
            return nil
        }
        if role == "Unknown", childNodes.isEmpty, !hasSemanticValue {
            return nil
        }

        return Wire.Node(
            role: role,
            identifier: identifier,
            label: label,
            value: value,
            frame: wireFrame,
            enabled: axValue(object, "accessibilityEnabled") as? Bool
                ?? true,
            selected: axValue(object, "accessibilitySelected") as? Bool
                ?? false,
            focused: axValue(object, "accessibilityFocused") as? Bool
                ?? false,
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
        return bounded(value, maximumBytes: maximumStringBytes)
    }

    /// Bound protocol strings by encoded size rather than grapheme count:
    /// one pathological Unicode grapheme can contain millions of bytes.
    private static func bounded(
        _ value: String, maximumBytes: Int
    ) -> String {
        let scalars = value.unicodeScalars
        var end = scalars.startIndex
        var byteCount = 0
        while end != scalars.endIndex {
            let byteWidth = scalars[end].utf8.count
            guard byteCount + byteWidth <= maximumBytes else {
                break
            }
            byteCount += byteWidth
            end = scalars.index(after: end)
        }
        return String(scalars[..<end])
    }

    private static func valueDescription(_ value: Any?) -> String? {
        switch value {
        case nil: return nil
        case let string as String: return nonEmpty(string)
        case let string as NSAttributedString:
            return nonEmpty(string.string)
        case let number as NSNumber: return nonEmpty(number.stringValue)
        default: return nil
        }
    }

    private static func finite(_ value: CGFloat) -> CGFloat {
        value.isFinite ? value : 0
    }

    private static func dimension(_ value: CGFloat) -> CGFloat {
        max(0, finite(value))
    }
}

#endif
