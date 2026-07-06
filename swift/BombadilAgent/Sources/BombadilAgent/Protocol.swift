import Foundation

/// Wire types for the newline-delimited JSON protocol between the
/// Bombadil driver and this agent. Must stay in sync with
/// `lib/bombadil-swiftui/src/agent.rs` and
/// `lib/bombadil-schema/src/swiftui.rs`.
enum Wire {
    static let protocolVersion: UInt32 = 1

    struct Frame: Codable, Equatable {
        var x: Double
        var y: Double
        var width: Double
        var height: Double
    }

    /// A node in the accessibility tree. The root represents the
    /// application; its children are windows. Frames use screen
    /// coordinates with the origin at the top-left.
    struct Node: Codable, Equatable {
        var role: String
        var identifier: String?
        var label: String?
        var value: String?
        var frame: Frame
        var enabled: Bool
        var selected: Bool
        var focused: Bool
        var children: [Node]
    }

    /// A message from the driver, decoded leniently from JSON.
    enum DriverMessage {
        case getState(quiescenceMillis: UInt64)
        case apply(Action)

        static func decode(_ data: Data) throws -> DriverMessage {
            let json = try JSONSerialization.jsonObject(with: data)
            guard let object = json as? [String: Any],
                let type = object["type"] as? String
            else {
                throw AgentError.protocolError("message without type: \(json)")
            }
            switch type {
            case "getState":
                let millis = object["quiescenceMillis"] as? UInt64 ?? 0
                return .getState(quiescenceMillis: millis)
            case "apply":
                guard let action = object["action"] as? [String: Any] else {
                    throw AgentError.protocolError("apply without action")
                }
                return .apply(try Action.decode(action))
            default:
                throw AgentError.protocolError("unknown message type: \(type)")
            }
        }
    }

    /// An action to perform, externally tagged: `{"Tap": {"x": …}}`.
    enum Action {
        case tap(x: Double, y: Double)
        case typeText(String)
        case pressKey(String)
        case scrollUp(x: Double, y: Double, distance: Double)
        case scrollDown(x: Double, y: Double, distance: Double)

        static func decode(_ object: [String: Any]) throws -> Action {
            func field(_ payload: Any?, _ name: String) throws -> Double {
                guard let payload = payload as? [String: Any],
                    let value = asDouble(payload[name])
                else {
                    throw AgentError.protocolError(
                        "action payload without \(name)")
                }
                return value
            }
            if let payload = object["Tap"] {
                return .tap(
                    x: try field(payload, "x"),
                    y: try field(payload, "y"))
            }
            if let payload = object["TypeText"] as? [String: Any],
                let text = payload["text"] as? String
            {
                return .typeText(text)
            }
            if let payload = object["PressKey"] as? [String: Any],
                let key = payload["key"] as? String
            {
                return .pressKey(key)
            }
            if let payload = object["ScrollUp"] {
                return .scrollUp(
                    x: try field(payload, "x"),
                    y: try field(payload, "y"),
                    distance: try field(payload, "distance"))
            }
            if let payload = object["ScrollDown"] {
                return .scrollDown(
                    x: try field(payload, "x"),
                    y: try field(payload, "y"),
                    distance: try field(payload, "distance"))
            }
            throw AgentError.protocolError("unknown action: \(object.keys)")
        }

        private static func asDouble(_ value: Any?) -> Double? {
            if let number = value as? NSNumber {
                return number.doubleValue
            }
            return value as? Double
        }
    }

    struct Hello: Encodable {
        let type = "hello"
        let protocolVersion: UInt32
    }

    struct State: Encodable {
        let type = "state"
        let root: Node
    }

    struct Applied: Encodable {
        let type = "applied"
    }

    struct ErrorReply: Encodable {
        let type = "error"
        let message: String
    }
}

enum AgentError: Error, CustomStringConvertible {
    case protocolError(String)
    case connectionError(String)
    case actionFailed(String)

    var description: String {
        switch self {
        case .protocolError(let message): return "protocol error: \(message)"
        case .connectionError(let message):
            return "connection error: \(message)"
        case .actionFailed(let message): return message
        }
    }
}
