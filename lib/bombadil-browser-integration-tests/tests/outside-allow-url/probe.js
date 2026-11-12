function probe(flag) {
  if (flag) {
    return 1;
  }
  return 2;
}
document.getElementById(document.currentScript.dataset.output).textContent =
  String(probe).includes("__bombadil__") ? "instrumented" : "as served";
