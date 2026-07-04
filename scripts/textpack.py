#!/usr/bin/env python3
"""Build a .textpack (zipped TextBundle) from a Markdown blog post.

The pack imports cleanly into Ulysses AND into Obsidian via the Omnighost
plugin's "Import textpack" command, which reads Ghost publishing metadata
from the bundle's info.json (the "omnighost" key below).

Layout produced:

    <name>.textbundle/
      text.markdown        # the post, prose reflowed to one line per paragraph
      info.json            # TextBundle v2 + {"omnighost": {blog, slug, tags, excerpt}}
      assets/<image>.png   # every local image the post references

Usage:

    textpack.py <post.md | post-dir> [options]

    --name NAME       bundle name (default: post dir name, or the .md stem)
    --blog DOMAIN     Ghost blog domain for Omnighost import (default: querygraph.ai)
    --slug SLUG       Ghost post slug (default: the bundle name)
    --tags a,b,c      Ghost tags
    --excerpt TEXT    Ghost excerpt
    --out FILE        output path (default: <post-dir>/dist/<name>.textpack)
    --no-reflow       keep the post's hard-wrapped lines as-is
    --render          re-render stale diagrams/*.mmd to PNG with mmdc first

The source post is never modified: reflow and the diagrams/->assets/ rewrite
apply only to the bundled copy. Mermaid sources live in <post-dir>/diagrams/
(one .mmd per diagram, PNG committed next to it).
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile

INFO_TYPE = "net.daringfireball.markdown"
# Local image refs the bundler collects: ![alt](diagrams/x.png), ![alt](assets/x.png),
# or any bare relative path without a scheme.
IMG_RE = re.compile(r"!\[([^\]]*)\]\(\s*(?!https?:|data:)([^)\s]+?\.(?:png|jpe?g|gif|webp|svg))\s*\)", re.I)
STRUCT_RE = re.compile(r"^(#|>|\||!\[|\s*[-*+] |\s*\d+\. |(---|\*\*\*|___)\s*$)")


def reflow(markdown: str) -> str:
    """Collapse each prose paragraph to one soft-wrapping line.

    Code fences, headings, lists, tables, blockquotes, images, and rules pass
    through untouched; only consecutive plain-prose lines are joined.
    """
    out, para, in_code = [], [], False

    def flush():
        if para:
            out.append(" ".join(para))
            para.clear()

    for ln in markdown.split("\n"):
        s = ln.strip()
        if s.startswith("```"):
            flush()
            out.append(ln)
            in_code = not in_code
            continue
        if in_code:
            out.append(ln)
            continue
        if s == "":
            flush()
            out.append("")
            continue
        if STRUCT_RE.match(s):
            flush()
            out.append(ln)
        else:
            para.append(s)
    flush()
    return "\n".join(out).rstrip("\n") + "\n"


def render_diagrams(post_dir: str) -> None:
    """Render stale diagrams/*.mmd to PNG with mmdc (white bg, 2x)."""
    ddir = os.path.join(post_dir, "diagrams")
    if not os.path.isdir(ddir):
        return
    if shutil.which("mmdc") is None:
        sys.exit("--render requested but mmdc (@mermaid-js/mermaid-cli) is not on PATH")
    for mmd in sorted(os.listdir(ddir)):
        if not mmd.endswith(".mmd"):
            continue
        src = os.path.join(ddir, mmd)
        png = src[:-4] + ".png"
        if os.path.exists(png) and os.path.getmtime(png) >= os.path.getmtime(src):
            continue
        print(f"rendering {mmd}")
        subprocess.run(["mmdc", "-i", src, "-o", png, "-b", "white", "-s", "2"], check=True)


def build(post_path: str, name: str, blog: str, slug: str, tags: list[str],
          excerpt: str, out: str, do_reflow: bool) -> str:
    post_dir = os.path.dirname(post_path)
    text = open(post_path, encoding="utf-8").read()

    if re.search(r"^```mermaid", text, re.M):
        print("WARNING: post contains fenced mermaid blocks; neither Ulysses nor Ghost "
              "renders them. Render to PNG (see diagrams/ + --render) and reference "
              "the images instead.", file=sys.stderr)

    if do_reflow:
        text = reflow(text)

    # Collect referenced local images and rewrite each ref to assets/<basename>.
    images: dict[str, str] = {}  # basename -> absolute source path
    missing: list[str] = []

    def to_asset(m: re.Match) -> str:
        alt, rel = m.group(1), m.group(2)
        src = os.path.normpath(os.path.join(post_dir, rel))
        base = os.path.basename(rel)
        if not os.path.isfile(src):
            missing.append(rel)
            return m.group(0)
        if base in images and images[base] != src:
            sys.exit(f"image basename collision in bundle: {base} "
                     f"({images[base]} vs {src}) — rename one of them")
        images[base] = src
        return f"![{alt}](assets/{base})"

    text = IMG_RE.sub(to_asset, text)
    if missing:
        sys.exit("missing image file(s): " + ", ".join(missing))

    info = {"version": 2, "type": INFO_TYPE, "transient": False}
    omnighost = {"blog": blog, "slug": slug}
    if tags:
        omnighost["tags"] = tags
    if excerpt:
        omnighost["excerpt"] = excerpt
    info["omnighost"] = omnighost

    with tempfile.TemporaryDirectory() as scratch:
        tb = os.path.join(scratch, f"{name}.textbundle")
        os.makedirs(os.path.join(tb, "assets"), exist_ok=True)
        with open(os.path.join(tb, "text.markdown"), "w", encoding="utf-8") as f:
            f.write(text)
        with open(os.path.join(tb, "info.json"), "w", encoding="utf-8") as f:
            json.dump(info, f, indent=2)
        for base, src in images.items():
            shutil.copy(src, os.path.join(tb, "assets", base))

        os.makedirs(os.path.dirname(out), exist_ok=True)
        if os.path.exists(out):
            os.remove(out)
        with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
            for root, _, files in os.walk(tb):
                for fn in sorted(files):
                    p = os.path.join(root, fn)
                    z.write(p, os.path.relpath(p, scratch))

    # The zip's top-level entry must be <name>.textbundle/ for Ulysses.
    with zipfile.ZipFile(out) as z:
        bad = [n for n in z.namelist() if not n.startswith(f"{name}.textbundle/")]
        if bad:
            sys.exit(f"zip layout wrong, entries outside {name}.textbundle/: {bad}")

    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("post", help="post .md file, or a directory containing post.md")
    ap.add_argument("--name")
    ap.add_argument("--blog", default="querygraph.ai")
    ap.add_argument("--slug")
    ap.add_argument("--tags", default="")
    ap.add_argument("--excerpt", default="")
    ap.add_argument("--out")
    ap.add_argument("--no-reflow", action="store_true")
    ap.add_argument("--render", action="store_true")
    args = ap.parse_args()

    post = args.post
    if os.path.isdir(post):
        post_dir = post.rstrip("/")
        post = os.path.join(post_dir, "post.md")
        default_name = os.path.basename(post_dir)
    else:
        post_dir = os.path.dirname(post) or "."
        stem = os.path.splitext(os.path.basename(post))[0]
        default_name = stem if stem != "post" else os.path.basename(os.path.abspath(post_dir))
    if not os.path.isfile(post):
        sys.exit(f"post not found: {post}")

    name = args.name or default_name
    slug = args.slug or name
    tags = [t.strip() for t in args.tags.split(",") if t.strip()]
    out = args.out or os.path.join(post_dir, "dist", f"{name}.textpack")

    if args.render:
        render_diagrams(post_dir)

    built = build(post, name, args.blog, slug, tags, args.excerpt, out, not args.no_reflow)
    size = os.path.getsize(built)
    print(f"built {built} ({size:,} bytes)")


if __name__ == "__main__":
    main()
