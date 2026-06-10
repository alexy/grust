import { mkdir, readFile, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import path from "node:path";

const root = path.resolve(import.meta.dirname);
const manuscript = path.join(root, "manuscript.md");
const buildDir = path.join(root, "build");
const diagramDir = path.join(buildDir, "diagrams");
const rendered = path.join(buildDir, "manuscript.rendered.md");
const puppeteerConfig = path.join(root, "puppeteer-config.json");

await mkdir(diagramDir, { recursive: true });

const source = await readFile(manuscript, "utf8");
let diagramIndex = 0;
const renderedMarkdown = source.replace(
  /```mermaid\n([\s\S]*?)\n```/g,
  (_match, diagram) => {
    diagramIndex += 1;
    const stem = `diagram-${String(diagramIndex).padStart(2, "0")}`;
    const input = path.join(diagramDir, `${stem}.mmd`);
    const output = path.join(diagramDir, `${stem}.png`);
    spawnSync("bash", ["-lc", `cat > "$1"`, "bash", input], {
      input: `${diagram.trim()}\n`,
      stdio: ["pipe", "inherit", "inherit"],
    });
    const result = spawnSync(
      "mmdc",
      ["-i", input, "-o", output, "-b", "transparent", "-p", puppeteerConfig, "-s", "2"],
      { stdio: "inherit" },
    );
    if (result.status !== 0) {
      throw new Error(`mmdc failed for ${input}`);
    }
    return `![Diagram ${diagramIndex}](diagrams/${stem}.png)`;
  },
);

await writeFile(rendered, renderedMarkdown);
console.log(`Rendered ${diagramIndex} Mermaid diagram(s) to ${rendered}`);
