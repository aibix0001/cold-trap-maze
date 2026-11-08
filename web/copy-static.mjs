// Copies the static files (HTML, CSS) next to the compiled scripts.
import { cpSync, mkdirSync } from "node:fs";
mkdirSync("dist", { recursive: true });
cpSync("static", "dist", { recursive: true });
console.log("static files copied to dist/");
