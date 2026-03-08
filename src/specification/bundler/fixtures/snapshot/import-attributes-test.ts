// Test import attributes with explicit type specifications
import textData from "./file.txt" with { type: "text" };
import jsonData from "./data.json" with { type: "json" };
import binaryData from "./binary.dat" with { type: "binary" };

export const textContent = textData;
export const jsonContent = jsonData;
export const binaryContent = binaryData;
