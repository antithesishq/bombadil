function type_text() {
  let element = document.activeElement;

  if (element === undefined || element === null || element === document.body) {
    return [];
  }

  if (element.tagName === "TEXTAREA") {
    return [
      [1, 50, { TypeText: { format: "Text" } }],
    ];
  }

  if (element.tagName === "INPUT") {
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
          [1, 50, { TypeText: { format: "Number" } }]
        ];

      case "color":
      default:
        return [];
    }
  }

  return [];
}
