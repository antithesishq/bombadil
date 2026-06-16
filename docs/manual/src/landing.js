(function () {
  var STORAGE_KEY = "bombadil-manual-driver";
  var select = document.getElementById("driver-select");
  var links = {
    read: document.getElementById("link-read"),
    install: document.getElementById("link-install"),
    pdf: document.getElementById("link-pdf"),
    epub: document.getElementById("link-epub"),
    txt: document.getElementById("link-txt"),
    man: document.getElementById("link-man"),
  };

  function updateLinks(driver) {
    links.read.href = driver + "/1-introduction.html";
    links.install.href = driver + "/2-getting-started.html#installation";
    links.pdf.href = driver + "/bombadil-manual.pdf";
    links.epub.href = driver + "/bombadil-manual.epub";
    links.txt.href = driver + "/bombadil-manual.txt";
    links.man.href = driver + "/bombadil.1";
  }

  var saved = null;
  try { saved = localStorage.getItem(STORAGE_KEY); } catch (_) {}
  var initial = saved && Array.from(select.options).some(function (o) { return o.value === saved; })
    ? saved
    : select.value;
  select.value = initial;
  updateLinks(initial);

  select.addEventListener("change", function () {
    updateLinks(select.value);
    try { localStorage.setItem(STORAGE_KEY, select.value); } catch (_) {}
  });
})();
