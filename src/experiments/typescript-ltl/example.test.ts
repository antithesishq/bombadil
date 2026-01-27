import { describe, it, expect } from "bun:test";
import { test } from "./test";
import assert from "node:assert";
import { condition, eventually, next, runtime_default } from "./bombadil";
import { ExtractorCell, Runtime } from "./runtime";
import { render_violation } from "./render";

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
    expect(result.violation.type).toEqual("always");
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

  it("file upload", async () => {
    type TestState = {
      file_name: string;
      spinner_visible: boolean;
      dialog_message: string | null;
      error_message: string | null;
      last_action: { id: "wait" | "upload_file" };
    };
    const trace = [
      {
        state: {
          file_name: "file.txt",
          spinner_visible: false,
          dialog_message: null,
          error_message: null,
          last_action: {
            id: "wait",
          },
        } satisfies TestState,
        timestamp_ms: 0,
      },
      {
        state: {
          file_name: "",
          spinner_visible: true,
          dialog_message: null,
          error_message: null,
          last_action: {
            id: "upload_file",
          },
        } satisfies TestState,
        timestamp_ms: 100,
      },
      {
        state: {
          file_name: "",
          spinner_visible: false,
          dialog_message: null,
          error_message: "Uh-oh, file too big!",
          last_action: {
            id: "wait",
          },
        } satisfies TestState,
        timestamp_ms: 4000,
      },
    ];
    const runtime = new Runtime<TestState>();
    const state = new ExtractorCell<TestState, TestState>(
      runtime,
      (state) => state,
    );

    const file_upload = condition(() => {
      const file_name = state.current.file_name.trim();
      return next(
        condition(
          () =>
            file_name !== "" &&
            state.current.spinner_visible &&
            state.current.last_action.id === "upload_file",
        ).implies(
          eventually(
            () =>
              state.current.dialog_message?.includes(file_name) ||
              !!state.current.error_message,
          ).within(5, "seconds"),
        ),
      );
    });

    const result = test(runtime, file_upload, trace);
    switch (result.type) {
      case "passed":
        return;
      case "failed":
        throw new Error("violation:\n\n" + render_violation(result.violation));
      case "inconclusive":
        throw new Error("formula should terminate");
    }
  });
});
