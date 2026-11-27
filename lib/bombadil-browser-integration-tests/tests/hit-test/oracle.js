// Records clicks on the top document's body: `data-landed` has the ids on the
// path of the clicks of the last press, `data-reached` every id clicked so far.
(function () {
  const data = window.top.document.body.dataset;
  data.landed ??= "";
  data.reached ??= "";
  document.addEventListener("mousedown", () => (data.landed = ""), true);
  document.addEventListener(
    "click",
    (event) => {
      for (const node of event.composedPath()) {
        if (!(node instanceof Element) || node.id === "") continue;
        data.landed += `${node.id} `;
        data.reached += `${node.id} `;
      }
    },
    true,
  );
})();
