# Grust Cover

The final cover is `grust-cover.png`. It uses the published Grust announcement
headboard as its visual source and the reusable First Pair Press publisher
mask from `~/src/firstpair/logo/firstpair-publisher-mask.png`.

- Source headboard: `grust-blog-headboard.png`
- Source URL: <https://digitalpress.fra1.cdn.digitaloceanspaces.com/cz6pt2z/2026/06/8FCCF44B-6852-441A-B787-07CD4B87C244.png>
- Generated portrait art: `grust-cover-art.png`
- Final composed cover: `grust-cover.png`

The portrait-art prompt was:

> Recompose the landscape Grust headboard as 2:3 portrait full-bleed cover
> art. Preserve the Soviet constructivist industrial-space language, weathered
> tan/red/black print texture, graph-like atomic structure, rockets,
> satellites, scaffolding, and workers. Remove all text and logos. Leave calm
> upper and lower regions for exact typography and a publisher mark. Add no
> new text or unrelated subjects.

The generated art intentionally contains no lettering. Exact title, subtitle,
author, and publisher-seal placement are reproducible with:

```sh
uv run --no-project --with pillow python cover/make-cover.py
```
