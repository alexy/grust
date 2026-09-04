# Preparing a Grust Blog TextPack

Grust owns each release post and its assets. FirstPair owns the shared,
provenance-aware TextPack implementation and delivery workflow. The
authoritative rules are in `~/src/firstpair/AGENTS.md`; this file records only
the Grust source layout and handoff.

## Source layout

```text
docs/blog/grust-<release>/
  post.md
  diagrams/                 # only when the post needs figures
    <figure>.mmd             # committed Mermaid source
    <figure>.png             # committed rendered asset
  dist/                     # generated after source is committed and pushed
    grust-<release>.textpack
    grust-<release> (<version>-<commit>).textpack
    VERSION.md
```

The canonical post should use one line per prose paragraph and reference local
images as `diagrams/<figure>.png`. Do not put raw Mermaid fences in a post that
will be handed to Ulysses or Ghost. Keep code fences, lists, tables, and image
lines structurally intact.

## Required sequence

1. Begin from the real Grust repository with a clean branch exactly synchronized
   with its upstream.
2. Edit `post.md` and every local asset. Render Mermaid sources to PNG with a
   white background and inspect the result.
3. Commit and push those finished source inputs.
4. From the clean, pushed Grust repository, run FirstPair's stamper:

   ```sh
   REPO_ROOT="$PWD" \
   BLOG_DOMAIN=querygraph.ai \
   BLOG_TAGS=rust,graphs,grust \
   BLOG_EXCERPT="Grust's latest backend-neutral property-graph release." \
   "$HOME/src/firstpair/publishing/scripts/stamp-versioned-blog.sh" \
     "docs/blog/grust-<release>"
   ```

5. Verify the generated archive, provenance, versioned link, and marker:

   ```sh
   unzip -t "docs/blog/grust-<release>/dist/grust-<release>.textpack"
   unzip -p "docs/blog/grust-<release>/dist/grust-<release>.textpack" \
     '*/info.json'
   cat "docs/blog/grust-<release>/dist/VERSION.md"
   git status --short
   ```

6. Commit and push the generated `dist/` handoff. The pack must use the
   `omnighost-textpack-v1` provenance schema and contain both `payloadSha256`
   and the full pushed source-changing Git commit.
7. Only after that second clean, pushed handoff may an authorized operator run
   FirstPair's `publish-versioned-blog.sh` to copy the already-built versioned
   archive to `~/icloud/blogs`.

Do not use the repository's former `scripts/textpack.py` shortcut for a release.
It produces a hash-only legacy pack and bypasses the current pushed-source
provenance gate. Do not build a release TextPack from dirty or merely local
inputs, and do not manually copy an unstamped pack to its delivery target.
