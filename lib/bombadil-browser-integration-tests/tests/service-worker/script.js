const count = document.getElementById("count");
count.textContent = "0";
document.getElementById("increment").addEventListener("click", () => {
    count.textContent = String(Number(count.textContent) + 1);
});
