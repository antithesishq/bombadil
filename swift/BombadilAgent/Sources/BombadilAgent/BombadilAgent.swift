import Foundation

#if os(macOS)
import AppKit
#endif

/// The in-app agent that lets Bombadil fuzz-test this app.
///
/// Add a call to `BombadilAgent.startIfRequested()` early in your app's
/// lifecycle, e.g. in your `App` initializer:
///
/// ```swift
/// @main
/// struct MyApp: App {
///     init() {
///         BombadilAgent.startIfRequested()
///     }
///     ...
/// }
/// ```
///
/// The call is a no-op unless the app was launched by
/// `bombadil swiftui test` (detected via the `BOMBADIL_SWIFTUI_CONNECT`
/// environment variable). Prefer linking and starting the agent only in
/// development builds.
public enum BombadilAgent {

    static let connectVariable = "BOMBADIL_SWIFTUI_CONNECT"

    /// Upper bound on how long a `getState` request waits for the UI
    /// to settle before answering with the latest tree anyway.
    static let quiescenceCap: TimeInterval = 1.0

    /// How long a single main-thread hop (a tree sample or an action)
    /// may take before the agent reports an error instead. Keep below
    /// the driver's state timeout so the driver sees a structured
    /// error, not a dead socket.
    static let mainThreadTimeout: TimeInterval = 5.0

    public static func startIfRequested() {
        guard
            let address = ProcessInfo.processInfo
                .environment[connectVariable]
        else {
            return
        }
        #if os(macOS)
        // Mark the app as having an assistive client attached, the
        // way VoiceOver does. Without this SwiftUI serves a stale
        // accessibility tree: the initial snapshot is readable, but
        // values stop updating. Deferred to the main queue so NSApp
        // exists even when called from the App initializer.
        DispatchQueue.main.async {
            let selector = NSSelectorFromString(
                "setAccessibilityEnhancedUserInterface:")
            if NSApp.responds(to: selector) {
                NSApp.setValue(
                    true, forKey: "accessibilityEnhancedUserInterface")
            }
        }
        let thread = Thread {
            do {
                try run(address: address)
            } catch {
                FileHandle.standardError.write(
                    Data("bombadil agent failed: \(error)\n".utf8))
            }
        }
        thread.name = "bombadil-agent"
        thread.start()
        #else
        FileHandle.standardError.write(
            Data(
                "bombadil agent: only macOS is supported at the moment\n"
                    .utf8))
        #endif
    }

    #if os(macOS)

    private static func run(address: String) throws {
        let parts = address.split(separator: ":")
        guard parts.count == 2, parts[0] == "127.0.0.1",
            let port = UInt16(parts[1])
        else {
            throw AgentError.connectionError(
                "\(connectVariable) must contain a 127.0.0.1 address")
        }
        let connection = try LineConnection(
            host: String(parts[0]), port: port)
        try connection.send(
            Wire.Hello(protocolVersion: Wire.protocolVersion))

        while let line = try connection.receiveLine() {
            let message: Wire.DriverMessage
            do {
                message = try Wire.DriverMessage.decode(line)
            } catch {
                try connection.send(Wire.ErrorReply(message: "\(error)"))
                continue
            }

            switch message {
            case .getState(let quiescenceMillis):
                let root: Wire.Node
                do {
                    root = try settledTree(
                        quiescence: TimeInterval(quiescenceMillis)
                            / 1000.0)
                } catch {
                    try connection.send(
                        Wire.ErrorReply(message: "\(error)"))
                    continue
                }
                try connection.send(Wire.State(root: root))
            case .apply(let action):
                do {
                    try MainThread.run(timeout: mainThreadTimeout) {
                        try ActionPerformer.perform(action)
                    }
                } catch {
                    try connection.send(Wire.ErrorReply(message: "\(error)"))
                    continue
                }
                try connection.send(Wire.Applied())
            }
        }
    }

    /// Samples the accessibility tree until two samples taken
    /// `quiescence` apart are identical (the UI has settled), bounded
    /// by `quiescenceCap`.
    private static func settledTree(
        quiescence: TimeInterval
    ) throws -> Wire.Node {
        var previous = try MainThread.run(timeout: mainThreadTimeout) {
            AccessibilityTree.snapshot()
        }
        guard quiescence > 0 else {
            return previous
        }
        let interval = min(quiescence, quiescenceCap)
        let deadline =
            ProcessInfo.processInfo.systemUptime + quiescenceCap
        while true {
            let remaining =
                deadline - ProcessInfo.processInfo.systemUptime
            guard remaining > 0 else {
                return previous
            }
            Thread.sleep(forTimeInterval: min(interval, remaining))
            let current = try MainThread.run(timeout: mainThreadTimeout) {
                AccessibilityTree.snapshot()
            }
            if current == previous {
                return current
            }
            previous = current
        }
    }

    #endif
}
