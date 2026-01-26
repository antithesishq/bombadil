import { describe, it, expect } from "bun:test";
import { test } from "./test";
import assert from "node:assert";
import { runtime_default } from "./bombadil";

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
  it("max notifications violation", async () => {
    const trace = [
      {
        state: new TestState({ ".notification": [new TestElement("DIV")] }),
        timestamp_ms: 0,
      },
      {
        state: new TestState({ ".notification": [new TestElement("DIV")] }),

        timestamp_ms: 1000,
      },
      {
        state: new TestState({
          // violation
          ".notification": new Array(6).fill(new TestElement("DIV")),
        }),
        timestamp_ms: 3000,
      },
    ];

    runtime_default.reset();
    const example = await import("./example");

    const result = test(
      runtime_default,
      example.max_notifications_shown,
      trace,
    );
    expect(result.type).toEqual("failed");

    assert(result.type === "failed");
    expect(result.violation.type).toEqual("false");
  });

  it("error disappears eventually", async () => {
    const trace = [
      { state: new TestState({ ".error": [] }), timestamp_ms: 0 },
      {
        state: new TestState({ ".error": [new TestElement("DIV")] }),
        timestamp_ms: 1000,
      },
      { state: new TestState({ ".error": [] }), timestamp_ms: 3000 }, // eventually satisfied
    ];

    runtime_default.reset();
    const example = await import("./example");

    const violation = test(runtime_default, example.error_disappears, trace);
    expect(violation.type).toBe("inconclusive");
  });

  it("error never disappears (still pending)", async () => {
    const trace = [
      {
        state: new TestState({ ".notification": [new TestElement("DIV")] }),
        timestamp_ms: 0,
      },
      {
        state: new TestState({ ".notification": [new TestElement("DIV")] }),
        timestamp_ms: 0,
      },
      {
        state: new TestState({ ".notification": [new TestElement("DIV")] }),
        timestamp_ms: 0,
      }, // still pending
    ];

    runtime_default.reset();
    const example = await import("./example");

    const violation = test(runtime_default, example.error_disappears, trace);
    expect(violation.type).toBe("inconclusive");
  });
});
