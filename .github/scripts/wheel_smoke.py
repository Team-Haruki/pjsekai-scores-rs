"""Smoke test of an installed pjsekai-scores-rs wheel: SVG, PNG, JPEG and raster output.

Run after installing the wheel (the maturin-wheels template's `test-command`). It needs
one TrueType font from the runner image, because the wheel has no system font fallback.
"""

import os
import sys

import pjsekai_scores_rs as pjs

CHART = """
#TITLE "Wheel smoke"
#ARTIST "Haruki"
#DIFFICULTY 3
#PLAYLEVEL 30
#BPM01: 120
#00008: 01
#00012: 14
#00014: 24
#00154: 34
#00216: 54
#00312: 1121314151617181
#00110: 11
"""

# (path, family) of a TrueType font present on the GitHub-hosted runner images.
FONTS = [
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "DejaVu Sans"),
    ("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", "Liberation Sans"),
    ("/System/Library/Fonts/Supplemental/Arial.ttf", "Arial"),
    ("C:\\Windows\\Fonts\\arial.ttf", "Arial"),
]


FONT_DIRS = ["/usr/share/fonts", "/System/Library/Fonts", "C:\\Windows\\Fonts"]


def find_font():
    font = next(((path, family) for path, family in FONTS if os.path.exists(path)), None)
    if font is not None:
        return font
    # Any TrueType font: with no family match the renderer falls back to it.
    for root in FONT_DIRS:
        for directory, _, files in os.walk(root):
            for name in sorted(files):
                if name.lower().endswith(".ttf"):
                    return os.path.join(directory, name), None
    return None


def main() -> int:
    font = find_font()
    if font is None:
        print("no TrueType font on this runner", file=sys.stderr)
        return 1
    path, family = font
    css = None
    if family is not None:
        css = (
            ".title, .subtitle, .bar-count-text, .event-text, .speed-text, .lyric-text, "
            f'.tick-text, .skill-text, .fever-text {{ font-family: "{family}"; }}\n'
        )

    assert pjs.RASTER_BACKEND == "tiny-skia", pjs.RASTER_BACKEND
    drawing = pjs.Drawing(
        score=pjs.Score.from_str(CHART),
        style_sheet=css,
        # An http note host draws vector notes, so no sprite assets are needed.
        note_host="https://assets.example.test/notes",
        font_paths=[path],
    )

    svg = drawing.svg()
    assert svg.lstrip().startswith("<svg"), svg[:40]

    png = drawing.png()
    assert png.startswith(b"\x89PNG\r\n\x1a\n"), png[:8]

    jpeg = drawing.jpeg(jpeg_quality=85)
    assert jpeg.startswith(b"\xff\xd8") and jpeg.endswith(b"\xff\xd9"), (jpeg[:2], jpeg[-2:])
    jpeg444 = drawing.jpeg(jpeg_quality=85, jpeg_subsampling="444")
    assert jpeg444.startswith(b"\xff\xd8") and len(jpeg444) >= len(jpeg)

    raster = drawing.raster()
    view = memoryview(raster)
    assert view.readonly
    assert raster.nbytes == raster.row_bytes * raster.height == view.nbytes
    assert raster.color_type == "rgba8888" and raster.alpha_type == "premul"

    print(
        f"wheel smoke OK with {path}: png {len(png)} B, jpeg {len(jpeg)} B, "
        f"raster {raster.width}x{raster.height}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
