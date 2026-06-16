(function () {
  var STORAGE_KEY = "bombadil-manual-driver";
  var selects = document.querySelectorAll(".driver-select");
  if (selects.length === 0) return;

  var current = selects[0].dataset.current;
  if (!current) return;

  selects.forEach(function (sel) { sel.value = current; });

  function switchTo(newDriver) {
    if (newDriver === current) return;
    try { localStorage.setItem(STORAGE_KEY, newDriver); } catch (_) {}
    var path = window.location.pathname;
    var marker = "/" + current + "/";
    var idx = path.lastIndexOf(marker);
    if (idx === -1) return;
    var newPath = path.slice(0, idx) + "/" + newDriver + "/" + path.slice(idx + marker.length);
    window.location.href = newPath + window.location.hash;
  }

  selects.forEach(function (sel) {
    sel.addEventListener("change", function () { switchTo(sel.value); });
  });
})();
