const count = document.getElementById("count");
document.getElementById("increment").addEventListener("click", () => {
  const next = Number(count.textContent) + 1;
  count.textContent = next % 2 === 0 ? `${next} (even)` : String(next);
});
