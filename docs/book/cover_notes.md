# Raster Cover Build Notes

Grust uses the same image-cover pattern as the current TypeSec and LakeCat
books.

## Source and Composition

Keep these assets under the repository-root `cover/` directory:

- the published blog headboard used as visual source;
- text-free 2:3 portrait artwork generated from that headboard;
- the reusable First Pair Press publisher mask;
- a deterministic Pillow composer for exact title, subtitle, author, and seal;
- the final 1024x1536 RGB PNG used by every book format;
- `README.md` with provenance, the image-generation prompt, and rebuild command.

Generated artwork should contain no lettering. The deterministic composer owns
the exact text and must keep `Alexy Khrabrov` as the sole author line.

## Format Integration

In `book.build.json`, set `pdf.coverImage` and `epub.coverImage` to the same
final PNG. Set `epub.includeRenderedCover` to `false` so Pandoc does not add a
second manuscript cover. Keep `docs/book/cover.md` as a small HTML image wrapper
for the browser readers.

The EPUB layout fixer must order the spine as Pandoc image cover, navigation
with `linear="no"`, then the first manuscript chapter. The validator should
check cover metadata, its 1024x1536 SVG wrapper, exact creator and publisher,
and byte identity between the packaged image and the canonical PNG.

The chapter HTML package must not retain a source-relative `cover/...` URL.
The shared FirstPair emitter resolves that URL through Pandoc's resource path,
copies the image to the chapter package's `assets/` directory, and rewrites the
reference.

## Visual Verification

After the canonical build succeeds, rasterize the first PDF page:

```sh
pdftoppm -f 1 -l 1 -singlefile -png -r 120 \
  docs/book/build/dist/grust.pdf /tmp/grust-cover
```

Inspect that PNG as well as the canonical cover. `pdftotext` cannot validate
lettering baked into a raster image.
