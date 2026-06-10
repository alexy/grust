```{=typst}
#set page(margin: 1in, numbering: none)
#set text(font: "Libertinus Serif", size: 13pt)

#align(center + horizon)[
  #block[
    #text(size: 32pt, weight: "bold", bottom-edge: "bounds")[{{title}}]

    #v(-12pt)

    #text(size: 10pt)[{{versionSubtitle}}]

    #v(1em)

    #text(size: 18pt)[{{subtitle}}]

    #v(3em)

    #text(size: 14pt)[{{author}}]

    #v(0.35em)

    #text(size: 11pt, style: "italic")[&]

    #v(0.35em)

    #text(size: 13pt, style: "italic")[{{coauthor}}]

  ]
]
```

```{=html}
<section class="cover-page" epub:type="cover">
  <div class="cover-title">{{title}}</div>
  <div class="cover-version">{{versionSubtitle}}</div>
  <div class="cover-subtitle">{{subtitle}}</div>
  <div class="cover-author">{{author}}</div>
  <div class="cover-credit-mark">&amp;</div>
  <div class="cover-coauthor">{{coauthor}}</div>
</section>
```
