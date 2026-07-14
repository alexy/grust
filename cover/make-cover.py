#!/usr/bin/env python3
"""Compose the Grust book cover from the generated portrait artwork."""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parent
ART = ROOT / "grust-cover-art.png"
MASK = ROOT / "firstpair-publisher-mask.png"
OUTPUT = ROOT / "grust-cover.png"

TITLE = "GRUST"
SUBTITLE = "A RUST PROPERTY GRAPH ARCHITECTURE"
AUTHOR = "ALEXY KHRABROV"

TITLE_FONT = Path("/System/Library/Fonts/Supplemental/DIN Condensed Bold.ttf")
TEXT_FONT = Path("/System/Library/Fonts/Supplemental/Arial Bold.ttf")

INK = (247, 235, 211, 255)
ACCENT = (174, 54, 33, 255)
PANEL = (34, 24, 18)
MARK_INK = (194, 169, 121)
MARK_ALPHA = 145


def font(path: Path, size: int) -> ImageFont.FreeTypeFont:
    if not path.is_file():
        raise SystemExit(f"missing cover font: {path}")
    return ImageFont.truetype(str(path), size=size)


def gradient_panel(size: tuple[int, int]) -> Image.Image:
    width, height = size
    alpha = Image.new("L", (1, height), 0)
    px = alpha.load()
    for y in range(height):
        top = round(222 * max(0.0, 1.0 - y / 650) ** 0.62)
        bottom = round(232 * max(0.0, (y - 1080) / (height - 1080)) ** 0.72)
        px[0, y] = max(top, bottom)
    alpha = alpha.resize((width, height))
    layer = Image.new("RGBA", size, PANEL + (0,))
    layer.putalpha(alpha)
    return layer


def tracked_width(draw: ImageDraw.ImageDraw, text: str, face: ImageFont.FreeTypeFont, tracking: int) -> float:
    return sum(draw.textlength(char, font=face) for char in text) + tracking * max(0, len(text) - 1)


def draw_tracked_center(
    draw: ImageDraw.ImageDraw,
    text: str,
    y: int,
    face: ImageFont.FreeTypeFont,
    fill: tuple[int, int, int, int],
    tracking: int,
) -> None:
    x = (draw._image.width - tracked_width(draw, text, face, tracking)) / 2
    for char in text:
        draw.text((x, y), char, font=face, fill=fill, stroke_width=2, stroke_fill=(0, 0, 0, 150))
        x += draw.textlength(char, font=face) + tracking


def wrap_lines(draw: ImageDraw.ImageDraw, text: str, face: ImageFont.FreeTypeFont, max_width: int) -> list[str]:
    lines: list[str] = []
    current = ""
    for word in text.split():
        candidate = word if not current else f"{current} {word}"
        if current and draw.textlength(candidate, font=face) > max_width:
            lines.append(current)
            current = word
        else:
            current = candidate
    if current:
        lines.append(current)
    return lines


def add_publisher_mark(canvas: Image.Image) -> None:
    source = Image.open(MASK).convert("L")
    alpha = source.point(lambda value: round(((value / 255) ** 0.76) * MARK_ALPHA))
    mark = Image.new("RGBA", source.size, MARK_INK + (0,))
    mark.putalpha(alpha)
    width = round(canvas.width * 0.25)
    height = round(mark.height * width / mark.width)
    mark = mark.resize((width, height), Image.Resampling.LANCZOS)
    canvas.alpha_composite(mark, ((canvas.width - width) // 2, canvas.height - height - 14))


def main() -> int:
    canvas = Image.open(ART).convert("RGBA")
    if canvas.size != (1024, 1536):
        raise SystemExit(f"expected 1024x1536 portrait artwork, got {canvas.size}")
    canvas.alpha_composite(gradient_panel(canvas.size))
    draw = ImageDraw.Draw(canvas)

    title_face = font(TITLE_FONT, 188)
    draw_tracked_center(draw, TITLE, 74, title_face, INK, 9)
    draw.rounded_rectangle((330, 280, 694, 288), radius=4, fill=ACCENT)

    subtitle_face = font(TEXT_FONT, 47)
    lines = wrap_lines(draw, SUBTITLE, subtitle_face, 830)
    line_height = 59
    for index, line in enumerate(lines):
        box = draw.textbbox((0, 0), line, font=subtitle_face, stroke_width=1)
        x = (canvas.width - (box[2] - box[0])) / 2
        draw.text((x, 318 + index * line_height), line, font=subtitle_face, fill=INK,
                  stroke_width=2, stroke_fill=(0, 0, 0, 150))

    author_face = font(TEXT_FONT, 42)
    draw_tracked_center(draw, AUTHOR, 1244, author_face, INK, 6)
    add_publisher_mark(canvas)

    canvas.convert("RGB").save(OUTPUT, optimize=True)
    print(OUTPUT)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
