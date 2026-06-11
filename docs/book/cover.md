```{=typst}
#set page(margin: 1in, numbering: none)
#align(center)[
  #v(28%)
  #text(size: 46pt, weight: "bold", bottom-edge: "bounds")[{{title}}]
  #v(-24pt)
  #text(size: 12pt)[{{versionSubtitle}}]
  #v(1em)
  #text(size: 20pt)[{{subtitle}}]
  #v(5em)
  #text(size: 20pt, weight: "medium")[{{author}}]
  #v(0.4em)
  #text(size: 13pt)[chiefscientist.org]
  #v(1.6em)
  #text(size: 11pt, style: "italic")[&]
  #v(0.35em)
  #text(size: 13pt, style: "italic")[{{coauthor}}]
]
```

```{=html}
<section class="cover-page" epub:type="titlepage">
  <div class="cover-title">{{title}}</div>
  <div class="cover-version">{{versionSubtitle}}</div>
  <div class="cover-subtitle">{{subtitle}}</div>
  <div class="cover-author">{{author}}</div>
  <div class="cover-author-site">chiefscientist.org</div>
  <div class="cover-credit-mark">&amp;</div>
  <div class="cover-coauthor">{{coauthor}}</div>
</section>
```
