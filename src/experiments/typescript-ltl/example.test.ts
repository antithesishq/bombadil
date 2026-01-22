import { describe, it, expect } from "bun:test";
import { test } from "./test";
import * as example from "./example";

class TestElement {
  constructor(public nodeName: string) {}

  querySelectorAll(_selector: string): HTMLElement[] {
    return [];
  }
  querySelector(_selector: string): HTMLElement | null {
    return null;
  }
}

class TestState {
  document: Document;

  constructor(private elements: Record<string, TestElement[]>) {
    const self = this;
    this.document = {
      get body() {
        return {
          querySelectorAll(selector: string) {
            return self.elements[selector] ?? [];
          },
          querySelector(selector: string) {
            return self.elements[selector]?.[0] ?? null;
          },
        } as unknown as HTMLBodyElement;
      },
    } as unknown as HTMLDocument;
  }
}

describe("LTL formula tests", () => {
  it("max notifications violation", () => {
    const trace = [
      new TestState({ ".notification": [new TestElement("DIV")] }),
      new TestState({ ".notification": [new TestElement("DIV")] }),
      new TestState({
        // violation
        ".notification": new Array(6).fill(new TestElement("DIV")),
      }),
    ];
    const result = test(example.max_notifications_shown, trace);
    expect(result).toEqual({
      type: "failed",
      violation: { time: 3, type: "false" },
    });
  });

  it("error disappears eventually", () => {
    const trace = [
      new TestState({ ".error": [] }),
      new TestState({ ".error": [new TestElement("DIV")] }),
      new TestState({ ".error": [] }), // eventually satisfied
    ];
    const violation = test(example.error_disappears, trace);
    expect(violation.type).toBe("inconclusive");
  });

  it("error never disappears (still pending)", () => {
    const trace = [
      new TestState({ ".notification": [new TestElement("DIV")] }),
      new TestState({ ".notification": [new TestElement("DIV")] }),
      new TestState({ ".notification": [new TestElement("DIV")] }), // still pending
    ];
    const violation = test(example.error_disappears, trace);
    expect(violation.type).toBe("inconclusive");
  });
});
