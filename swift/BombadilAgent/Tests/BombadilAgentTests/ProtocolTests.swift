import Foundation
import XCTest

@testable import BombadilAgent

final class ProtocolTests: XCTestCase {
    private func decode(_ json: String) throws -> Wire.DriverMessage {
        try Wire.DriverMessage.decode(Data(json.utf8))
    }

    func testDecodesValidStateRequest() throws {
        let message = try decode(
            #"{"type":"getState","quiescenceMillis":100}"#)
        guard case .getState(let millis) = message else {
            return XCTFail("expected getState")
        }
        XCTAssertEqual(millis, 100)
    }

    func testAllowsNegativeGlobalTapCoordinates() throws {
        let message = try decode(
            """
            {"type":"apply","action":{"Tap":{"x":-200.5,"y":-20}}}
            """)
        guard case .apply(let action) = message,
            case .tap(let x, let y) = action
        else {
            return XCTFail("expected tap")
        }
        XCTAssertEqual(x, -200.5)
        XCTAssertEqual(y, -20)
    }

    func testRejectsBooleanCoordinates() {
        XCTAssertThrowsError(
            try decode(
                """
                {"type":"apply","action":{"Tap":{"x":true,"y":20}}}
                """))
    }

    func testRejectsFractionalQuiescenceMilliseconds() {
        XCTAssertThrowsError(
            try decode(
                #"{"type":"getState","quiescenceMillis":1.5}"#))
    }

    func testRejectsMultipleActionVariants() {
        XCTAssertThrowsError(
            try decode(
                """
                {"type":"apply","action":{
                  "Tap":{"x":10,"y":20},
                  "PressKey":{"key":"return"}
                }}
                """))
    }

    func testRejectsUnknownActionFields() {
        XCTAssertThrowsError(
            try decode(
                """
                {"type":"apply","action":{
                  "Tap":{"x":10,"y":20,"z":30}
                }}
                """))
    }

    func testRejectsNegativeScrollDistances() {
        XCTAssertThrowsError(
            try decode(
                """
                {"type":"apply","action":{
                  "ScrollDown":{"x":10,"y":20,"distance":-1}
                }}
                """))
    }

    func testRejectsUnrepresentableScrollDistances() {
        XCTAssertThrowsError(
            try decode(
                """
                {"type":"apply","action":{
                  "ScrollUp":{"x":10,"y":20,"distance":2147483648}
                }}
                """))
    }
}
