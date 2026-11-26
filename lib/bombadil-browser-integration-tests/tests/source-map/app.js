function label(count) { return count > 1 ? "clicks" : "click"; }
let count = 0; document.getElementById("button").addEventListener("click", () => { count += 1; if (count > 0) { document.getElementById("output").textContent = count + " " + label(count); } });
//# sourceMappingURL=app.js.map
