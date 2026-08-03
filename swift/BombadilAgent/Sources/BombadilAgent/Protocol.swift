import Foundation

/// Wire types for the newline-delimited JSON protocol between the
/// Bombadil driver and this agent. Must stay in sync with
/// `lib/bombadil-swiftui/src/agent.rs` and
/// `lib/bombadil-schema/src/swiftui.rs`.
enum Wire {
    static let protocolVersion: UInt32 = 1

    struct Frame: Codable, Equatable, Sendable {
        var x: Double
        var y: Double
        var width: Double
        var height: Double
    }

    /// A node in the accessibility tree. The root represents the
    /// application; its children are windows. Frames use screen
    /// coordinates with the origin at the top-left.
    struct Node: Codable, Equatable, Sendable {
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

    enum DriverMessage: Sendable {
        case getState(quiescenceMillis: UInt64)
        case apply(Action)

        static func decode(_ data: Data) throws -> DriverMessage {
            let json = try JSONSerialization.jsonObject(with: data)
            guard let object = json as? [String: Any],
                let type = object["type"] as? String
            else {
                throw AgentError.protocolError(
                    "message must be an object with a string type")
            }
            switch type {
            case "getState":
                guard
                    Set(object.keys) == ["type", "quiescenceMillis"],
                    let millis = Wire.asUInt64(
                        object["quiescenceMillis"])
                else {
                    throw AgentError.protocolError(
                        "invalid getState payload")
                }
                return .getState(quiescenceMillis: millis)
            case "apply":
                guard
                    Set(object.keys) == ["type", "action"],
                    let action = object["action"] as? [String: Any]
                else {
                    throw AgentError.protocolError("invalid apply payload")
                }
                return .apply(try Action.decode(action))
            default:
                throw AgentError.protocolError("unknown message type")
            }
        }
    }

    /// An action to perform, externally tagged: `{"Tap": {"x": …}}`.
    enum Action: Sendable {
        case tap(x: Double, y: Double)
        case typeText(String)
        case pressKey(String)
        case scrollUp(x: Double, y: Double, distance: Double)
        case scrollDown(x: Double, y: Double, distance: Double)

        static func decode(_ object: [String: Any]) throws -> Action {
            guard object.count == 1 else {
                throw AgentError.protocolError(
                    "action must contain exactly one variant")
            }

            func payload(
                _ name: String, fields: Set<String>
            ) throws -> [String: Any]? {
                guard let value = object[name] else {
                    return nil
                }
                guard let value = value as? [String: Any],
                    Set(value.keys) == fields
                else {
                    throw AgentError.protocolError(
                        "invalid \(name) payload")
                }
                return value
            }

            func field(
                _ payload: [String: Any], _ name: String
            ) throws -> Double {
                guard let value = Wire.asDouble(payload[name]) else {
                    throw AgentError.protocolError(
                        "invalid numeric field \(name)")
                }
                return value
            }

            func distance(_ payload: [String: Any]) throws -> Double {
                let value = try field(payload, "distance")
                guard value >= 0, value <= Double(Int32.max) else {
                    throw AgentError.protocolError(
                        "scroll distance is out of range")
                }
                return value
            }

            if let payload = try payload("Tap", fields: ["x", "y"]) {
                return .tap(
                    x: try field(payload, "x"),
                    y: try field(payload, "y"))
            }
            if let payload = try payload("TypeText", fields: ["text"]),
                let text = payload["text"] as? String
            {
                return .typeText(text)
            }
            if let payload = try payload("PressKey", fields: ["key"]),
                let key = payload["key"] as? String
            {
                return .pressKey(key)
            }
            if let payload = try payload(
                "ScrollUp", fields: ["x", "y", "distance"])
            {
                return .scrollUp(
                    x: try field(payload, "x"),
                    y: try field(payload, "y"),
                    distance: try distance(payload))
            }
            if let payload = try payload(
                "ScrollDown", fields: ["x", "y", "distance"])
            {
                return .scrollDown(
                    x: try field(payload, "x"),
                    y: try field(payload, "y"),
                    distance: try distance(payload))
            }
            throw AgentError.protocolError("unknown action")
        }
    }

    struct Hello: Encodable, Sendable {
        let type = "hello"
        let protocolVersion: UInt32
    }

    struct State: Encodable, Sendable {
        let type = "state"
        let root: Node
    }

    struct Applied: Encodable, Sendable {
        let type = "applied"
    }

    struct ErrorReply: Encodable, Sendable {
        let type = "error"
        let message: String
    }

    private static func jsonNumber(_ value: Any?) -> NSNumber? {
        guard let number = value as? NSNumber,
            CFGetTypeID(number) != CFBooleanGetTypeID()
        else {
            return nil
        }
        return number
    }

    private static func asUInt64(_ value: Any?) -> UInt64? {
        guard let number = jsonNumber(value) else {
            return nil
        }
        return UInt64(number.stringValue)
    }

    private static func asDouble(_ value: Any?) -> Double? {
        guard let number = jsonNumber(value) else {
            return nil
        }
        let value = number.doubleValue
        return value.isFinite ? value : nil
    }
}

enum AgentError: Error, CustomStringConvertible, Sendable {
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
