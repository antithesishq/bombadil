// External ES module script
export function setMessage() {
    const output = document.getElementById("output");
    if (output) {
        output.textContent = "External ES module loaded successfully";
    }
    console.log("External ES module loaded successfully");
}

// Auto-execute
setMessage();
