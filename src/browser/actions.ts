export type Point = {
  x: number,
  y: number,
};

export type Action = {
  Click: {
    name: string,
    content: string,
    point: Point,
  }
} | {
  TypeText: {
    format: "Text" | "Email" | "Number"
  }
}
  | "PressKey"
  | {
    ScrollUp: {
      origin: {
        x: number,
        y: number,
      },
      distance: number,
    }
  }
  | {
    ScrollDown: {
      origin: {
        x: number,
        y: number,
      },
      distance: number,
    }
  };

export type Weight = number;

export type Timeout = number;

export type Actions = [Weight, Timeout, Action][];

/// Like document.querySelectorAll, but searches recursively down into shadow roots and iframes.
export function query_all(root: Element, selector: string): Element[] {
  const queue = [root];
  const results = [];

  while (queue.length > 0) {
    const element = queue.pop();
    if (element === undefined) {
      break;
    }

    if (element.matches(selector)) {
      results.push(element);
    }

    if (element.shadowRoot) {
      for (const child of element.shadowRoot.children) {
        queue.push(child);
      }
    } else if (element instanceof HTMLIFrameElement && element.contentDocument) {
      queue.push(element.contentDocument.body);
    } else {

      for (const child of element.children) {
        queue.push(child);
      }
    }
  }

  return results;
}
