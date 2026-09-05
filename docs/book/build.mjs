import { mkdir, readFile, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname);
const workspaceRoot = path.resolve(root, "../..");
const cargoToml = path.join(workspaceRoot, "crates", "grust", "Cargo.toml");
const metadata = path.join(root, "metadata.yaml");
const cover = path.join(root, "cover.md");
const manuscript = path.join(root, "manuscript.md");
const buildDir = path.join(root, "build");
const diagramDir = path.join(buildDir, "diagrams");
const renderedCover = path.join(buildDir, "cover.rendered.md");
const rendered = path.join(buildDir, "manuscript.rendered.md");
const puppeteerConfig = path.join(root, "puppeteer-config.json");

await mkdir(diagramDir, { recursive: true });

const readYamlString = (yaml, key) => {
  const match = yaml.match(new RegExp(`^${key}:\\s*"([^"]+)"\\s*$`, "m"));
  if (!match) {
    throw new Error(`Missing ${key} in ${metadata}`);
  }
  return match[1];
};

const cargoSource = await readFile(cargoToml, "utf8");
const packageTable = cargoSource.match(/\[package\]([\s\S]*?)(?:\n\[|$)/);
const version = packageTable?.[1].match(/^\s*version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) {
  throw new Error(`Missing explicit facade [package] version in ${cargoToml}`);
}

const metadataSource = await readFile(metadata, "utf8");
const titleStem = readYamlString(metadataSource, "title_stem");
const coverValues = {
  title: readYamlString(metadataSource, "title"),
  titleStem,
  subtitle: readYamlString(metadataSource, "subtitle"),
  author: readYamlString(metadataSource, "author"),
  rights: readYamlString(metadataSource, "rights"),
  versionSubtitle: `covers ${titleStem} (${version})`,
};

const escapeHtml = (value) =>
  value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const escapeTypstMarkup = (value) =>
  value.replace(/\\/g, "\\\\").replace(/\[/g, "\\[").replace(/\]/g, "\\]");

const coverSource = await readFile(cover, "utf8");
const renderedCoverMarkdown = coverSource.replace(
  /\{\{(title|subtitle|author|rights|versionSubtitle)\}\}/g,
  (match, key, offset) => {
    const before = coverSource.slice(0, offset);
    const typstFence = before.lastIndexOf("```{=typst}");
    const htmlFence = before.lastIndexOf("```{=html}");
    const markdownFence = before.lastIndexOf("```");
    const value = coverValues[key];
    if (typstFence > htmlFence && typstFence === markdownFence) {
      return escapeTypstMarkup(value);
    }
    if (htmlFence > typstFence && htmlFence === markdownFence) {
      return escapeHtml(value);
    }
    return value;
  },
);
await writeFile(renderedCover, renderedCoverMarkdown);

const source = await readFile(manuscript, "utf8");
let diagramIndex = 0;
const renderedMarkdown = source.replace(
  /```mermaid\n([\s\S]*?)\n```/g,
  (_match, diagram) => {
    diagramIndex += 1;
    const stem = `diagram-${String(diagramIndex).padStart(2, "0")}`;
    const input = path.join(diagramDir, `${stem}.mmd`);
    const output = path.join(diagramDir, `${stem}.png`);
    writeFileSync(input, `${diagram.trim()}\n`);
    const result = spawnSync(
      process.execPath,
      [path.join(root, "render-mermaid.mjs"), input, output, puppeteerConfig],
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
console.log(`Rendered cover for ${coverValues.titleStem} (${version}) to ${renderedCover}`);
