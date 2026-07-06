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
/// environment variable), so it is safe to keep in release builds or
/// behind your own `#if DEBUG` guard.
public enum BombadilAgent {

    static let connectVariable = "BOMBADIL_SWIFTUI_CONNECT"

    /// Upper bound on how long a `getState` request waits for the UI
    /// to settle before answering with the latest tree anyway.
    static let quiescenceCap: TimeInterval = 1.0

    public static func startIfRequested() {
        guard
            let address = ProcessInfo.processInfo
                .environment[connectVariable]
        else {
            return
        }
        #if os(macOS)
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
        guard parts.count == 2, let port = UInt16(parts[1]) else {
            throw AgentError.connectionError(
                "malformed \(connectVariable): \(address)")
        }
        let connection = try LineConnection(
            host: String(parts[0]), port: port)
        try connection.send(
            Wire.Hello(protocolVersion: Wire.protocolVersion))

        while let line = try connection.receiveLine() {
            switch try Wire.DriverMessage.decode(line) {
            case .getState(let quiescenceMillis):
                let root = settledTree(
                    quiescence: TimeInterval(quiescenceMillis) / 1000.0)
                try connection.send(Wire.State(root: root))
            case .apply(let action):
                do {
                    try onMain { try ActionPerformer.perform(action) }
                    try connection.send(Wire.Applied())
                } catch {
                    try connection.send(Wire.ErrorReply(message: "\(error)"))
                }
            }
        }
    }

    /// Samples the accessibility tree until two samples taken
    /// `quiescence` apart are identical (the UI has settled), bounded
    /// by `quiescenceCap`.
    private static func settledTree(quiescence: TimeInterval) -> Wire.Node {
        var previous = onMain { AccessibilityTree.snapshot() }
        guard quiescence > 0 else {
            return previous
        }
        let deadline = Date().addingTimeInterval(quiescenceCap)
        while Date() < deadline {
            Thread.sleep(forTimeInterval: quiescence)
            let current = onMain { AccessibilityTree.snapshot() }
            if current == previous {
                return current
            }
            previous = current
        }
        return previous
    }

    private static func onMain<T>(_ body: () throws -> T) rethrows -> T {
        if Thread.isMainThread {
            return try body()
        }
        return try DispatchQueue.main.sync(execute: body)
    }

    #endif
}
