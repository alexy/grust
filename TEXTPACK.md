# Preparing a Ulysses TextPack from a Grust blog post

How to turn a Markdown blog post that uses fenced `mermaid` diagrams (e.g.
`docs/blog/grust-<release>/post.md`) into a self-contained **`.textpack`** that
imports cleanly into Ulysses — including on iOS, where external image paths and
`mermaid` code blocks do not render.

A `.textpack` is the right deliverable because it bundles the Markdown text *and*
the image assets into one importable package. Pasting raw Markdown into Ulysses
(or Ghost) instead tends to produce two problems this guide also fixes:

- **Ragged lines with big vertical gaps** — caused by hard-wrapped prose; the
  editor treats every newline as a line break. Fix: reflow to one line per
  paragraph.
- **Missing diagrams** — Ulysses/Ghost do not render `mermaid`. Fix: pre-render
  diagrams to PNG and reference the images.

## Format

A TextBundle is a folder; a TextPack is that folder zipped:

```
<name>.textbundle/
  text.markdown          # the post (Markdown / Markdown XL)
  info.json              # {"version":2,"type":"net.daringfireball.markdown","transient":false}
  assets/<diagram>.png   # bundled images, referenced as assets/<diagram>.png
```

Zip the `.textbundle` directory (with the directory as the top-level entry) to
`<name>.textpack`. Ulysses imports the `.textpack` via the share sheet or
**＋ → Import**.

## Prerequisites

- `mmdc` — the Mermaid CLI (`@mermaid-js/mermaid-cli`). Renders fenced mermaid to
  PNG. No puppeteer config is normally needed; if Chrome sandbox errors appear,
  pass `--puppeteerConfigFile docs/book/puppeteer-config.json` (it sets
  `--no-sandbox`).
- `python3` — for the reflow and bundling steps below (no third-party packages).

## Steps

### 1. Reflow prose to one line per paragraph

Hard wrapping is what makes the text render ragged with paragraph gaps. Collapse
each prose paragraph to a single soft-wrapping line; leave code fences, lists,
headings, blockquotes, tables, and image lines untouched.

```python
import re
src = "docs/blog/grust-crab/post.md"
lines = open(src).read().split("\n")
out, para, in_code = [], [], False
def flush():
    if para: out.append(" ".join(para)); para.clear()
struct = re.compile(r"^(#|>|\||!\[|\s*[-*+] |\s*\d+\. |(---|\*\*\*|___)\s*$)")
for ln in lines:
    s = ln.strip()
    if s.startswith("```"):
        flush(); out.append(ln); in_code = not in_code; continue
    if in_code: out.append(ln); continue          # code verbatim
    if s == "": flush(); out.append(""); continue # blank = paragraph break
    if struct.match(s): flush(); out.append(ln)   # structural line: keep as-is
    else: para.append(s)                           # prose: accumulate
flush()
open(src, "w").write("\n".join(out).rstrip("\n") + "\n")
```

Sanity checks: the fence count (`grep -c '```'`) must be unchanged, and code
blocks must remain multi-line.

### 2. Render the diagrams to PNG

Keep `mermaid` sources in a `diagrams/` directory (one `.mmd` per diagram, synced
with the post). Render each at 2× on a **white** background (safe for both
light and dark editors):

```sh
cd docs/blog/grust-crab
for n in diagrams/*.mmd; do
  mmdc -i "$n" -o "${n%.mmd}.png" -b white -s 2
done
```

If you edit a diagram's content (e.g. adding a new component to the architecture
map), edit the `.mmd` source and re-render. To extract the post's inline
`mermaid` blocks back into `.mmd` files (so source and rendered images stay in
sync), classify each block by a keyword and write it out before rendering.

### 3. Point the post at the images

In the canonical post, replace each inline `mermaid` block with an image
reference (`![caption](diagrams/<name>.png)`). For the TextPack the bundler
rewrites `diagrams/...` to `assets/...` (next step), so the repo post keeps the
`diagrams/` path and the bundle is self-contained.

### 4. Build the `.textpack`

```python
import re, os, json, zipfile, shutil
base = "docs/blog/grust-crab"
post = open(f"{base}/post.md").read()
ddir = f"{base}/diagrams"
scratch = "/tmp"                               # temporary .textbundle workspace
dist = f"{base}/dist"; os.makedirs(dist, exist_ok=True)  # committed output, next to the post
tb   = f"{scratch}/grust-crab.textbundle"
shutil.rmtree(tb, ignore_errors=True); os.makedirs(f"{tb}/assets", exist_ok=True)
imgs = set(re.findall(r"!\[[^\]]*\]\(diagrams/([a-z0-9-]+\.png)\)", post))
text = re.sub(r"\(diagrams/([a-z0-9-]+\.png)\)", r"(assets/\1)", post)  # diagrams/ -> assets/
open(f"{tb}/text.markdown", "w").write(text)
json.dump({"version": 2, "type": "net.daringfireball.markdown", "transient": False},
          open(f"{tb}/info.json", "w"))
for n in imgs: shutil.copy(f"{ddir}/{n}", f"{tb}/assets/{n}")
pack = f"{dist}/grust-crab.textpack"
if os.path.exists(pack): os.remove(pack)
with zipfile.ZipFile(pack, "w", zipfile.ZIP_DEFLATED) as z:
    for root, _, files in os.walk(tb):
        for fn in files:
            p = os.path.join(root, fn); z.write(p, os.path.relpath(p, scratch))
```

The `.textpack` is committed at `docs/blog/<name>/dist/<name>.textpack` next to
the post; the `.textbundle` is a temporary workspace (build it under `/tmp`).

The zip's top entry must be `<name>.textbundle/` (verify with
`zipfile.ZipFile(pack).namelist()`).

## Fallback: a single self-contained Markdown file

If a `.textpack` is inconvenient, embed the PNGs as base64 data URIs in one
Markdown file (`![alt](data:image/png;base64,...)`). It is fully self-contained
but heavier, and not every editor renders data-URI images — the `.textpack` is
the more reliable bundle for Ulysses.

## Gotchas

- **Reflow first.** Ragged lines / vertical gaps are a hard-wrapping artifact, not
  a Ulysses/Ghost bug.
- **Render mermaid.** Neither Ulysses nor Ghost renders `mermaid` blocks; ship PNGs.
- **White background, 2× scale** for crisp, paste-anywhere images.
- **iOS:** relative image paths in pasted Markdown do not resolve — only the
  bundled `.textpack` (or base64) shows images inline.
- **Keep the `.textpack` in `dist/`.** Commit the built `.textpack` at
  `docs/blog/<name>/dist/<name>.textpack` next to the post, so it ships in the open
  alongside `post.md` + `diagrams/`. The intermediate `.textbundle` is scratch
  (build under `/tmp`), and the base64 `.md` fallback stays an ad-hoc deliverable.

## Relation to releases

Per `AGENTS.md`, each release ships a blog post at
`docs/blog/grust-<release>/post.md` with diagrams under `diagrams/`. This guide is
the last-mile step to hand that post to a writing/publishing app.
