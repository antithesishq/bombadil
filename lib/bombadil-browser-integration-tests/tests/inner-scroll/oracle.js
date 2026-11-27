// Records, on the top document's body, the button each press really lands
// on (`data-last`, and all of them in `data-hits`) and the number of presses
// (`data-presses`).
document.addEventListener(
  "mousedown",
  (event) => {
    const data = window.top.document.body.dataset;
    const button = event
      .composedPath()
      .find((node) => node instanceof HTMLButtonElement);
    const name = button?.id ?? "none";
    data.last = name;
    data.presses = Number(data.presses ?? 0) + 1;
    data.hits = `${data.hits ?? ""}${name} `;
  },
  true,
);

// Appends `count` buttons named `prefix-0`, `prefix-1`, ... and expects a
// press on each.
function addButtons(container, prefix, count) {
  const data = window.top.document.body.dataset;
  for (let i = 0; i < count; i++) {
    const button = document.createElement("button");
    button.id = `${prefix}-${i}`;
    button.textContent = button.id;
    container.appendChild(button);
    data.expect = `${data.expect ?? ""}${button.id} `;
  }
}
