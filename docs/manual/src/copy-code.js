document.addEventListener("click", function(e) {
  const btn = e.target.closest(".code-block .copy");
  const icon = e.target.querySelector(".code-block .icon");
  if (!btn || !icon) return;

  const block = btn.closest(".code-block");
  const source = block.querySelector(".sourceCode");
  if (!source) return;

  navigator.clipboard.writeText(source.textContent).then(function() {

    if (icon.textContent !== "✓") {
      const contentOld = icon.textContent;
      icon.classList.add("copied");
      icon.textContent = "✓";
      setTimeout(function() {
        icon.classList.remove("copied");
        icon.textContent = contentOld;
      }, 1500);
    }
  });
});
