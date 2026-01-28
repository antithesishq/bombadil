import { Actions } from "../actions";

result = (() => {
  let element = document.activeElement;

  if (element === undefined || element === null || element === document.body) {
    return [];
  }

  if (element instanceof HTMLTextAreaElement) {
    return [[1, 50, { TypeText: { format: "Text" } }]];
  }

  if (element instanceof HTMLInputElement) {
    switch (element.type) {
      case "text":
        return [
          [1, 50, "PressKey"],
          [1, 50, { TypeText: { format: "Text" } }],
        ];
      case "email":
        return [
          [1, 50, "PressKey"],
          [1, 50, { TypeText: { format: "Email" } }],
        ];
      case "number":
        return [
          [1, 50, "PressKey"],
          [1, 50, { TypeText: { format: "Number" } }],
        ];

      case "color":
      default:
        return [];
    }
  }

  return [];
})() satisfies Actions;
