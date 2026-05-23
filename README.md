# pjsekai-scores-rs

Project SEKAI score (`.sus`) parser, SVG chart renderer, and direct Skia image renderer, rewritten in Rust.

- Original Python project: [pjsekai/scores](https://gitlab.com/pjsekai/scores)
- Skill information previewer based on [xfl03's fork](https://github.com/xfl03/SekaiMusicChart)

## Features

- Parses `.sus` score files and Project SEKAI custom chart JSON
- Generates SVG chart images with full note rendering (Tap / Directional / Slide)
- Generates PNG/JPEG chart images directly with Skia, without SVG rasterization
- Honors CSS `font-family` in Skia image output, with custom font files/directories for CJK and JP fonts
- BPM rebase support (custom timing via JSON)
- Lyric overlay support
- Skill/Fever cover overlay support
- Dual distribution: **Rust crate** and **Python wheel** (via PyO3)
- Python 3.13 / 3.14t (free-threaded) compatible

## Performance

Benchmarked against the original Python implementation (`pjsekai/scores`) on the same input (`.sus` → SVG, 30 iterations, median):

| Phase | Python | Rust | Speedup |
|---|---|---|---|
| Parse (`.sus` → `Score`) | 14.454ms | 3.265ms | **4.4×** |
| Render (`Score` → SVG string) | 382.670ms | 20.163ms | **19.0×** |
| **Total** | **404.806ms** | **23.229ms** | **17.4×** |

> **Environment:** Mac mini M4 · macOS 26.4.1 · Python 3.13 · ARM64

The dominant win is SVG generation: Rust replaces thousands of Python `svgwrite` object allocations and DOM traversals with direct `String` building and pre-computed layout arithmetic.

---

## CLI Usage

```
pjsekai-scores-rs <SCORE> [OPTIONS]

Arguments:
  <SCORE>  The score file (.sus or Project SEKAI custom chart JSON)

Options:
      --score-format <SCORE_FORMAT>
                                 Input score format [default: auto] [possible values: auto, sus, json]
      --rebase <REBASE>          Customized BPM, beats and sections (JSON)
      --lyric <LYRIC>            Lyrics file
      --css <CSS>                Custom CSS stylesheet
      --note-host <NOTE_HOST>    Base URL for SVG note assets, or local directory for Skia image note assets
                                 [default: https://asset3.pjsekai.moe/live/note/custom01]
      --note-asset-extension <EXTENSION>
                                 File extension for note asset files [default: png]
      --font-path <FONT_PATH>    Font file path to load for Skia image output; may be repeated
      --font-dir <FONT_DIR>      Directory containing .ttf/.otf/.ttc fonts for Skia image output; may be repeated
      --title <TITLE>            Music title shown in the chart footer
      --artist <ARTIST>          Music artist shown in the chart footer
      --difficulty <DIFFICULTY>  Difficulty shown in the chart footer
      --play-level <PLAY_LEVEL>  Play level shown in the chart footer
      --music-id <MUSIC_ID>      Music ID shown in the chart footer
      --jacket <JACKET>          Jacket image URI/path shown in the chart footer
      --skill                    Render skill and fever overlay coverage
      --music-meta <MUSIC_META>  Music metadata JSON or JSON file path for skill score overlay
      --jpeg-quality <JPEG_QUALITY>
                                 JPEG quality for .jpg/.jpeg output (0-100) [default: 90]
      --perf                     Print render/write timing statistics
      --generator <GENERATOR>    Generator name shown in the SVG subtitle
                                 [default: HarukiBot NEO]
  -o, --output <OUTPUT>          Output file path (.svg, .png, .jpg, or .jpeg)
  -h, --help                     Print help
```

### Example

```bash
# Basic conversion
pjsekai-scores-rs master.sus -o master.svg

# Project SEKAI custom chart JSON is auto-detected from .json files.
pjsekai-scores-rs custom-chart.json -o custom-chart.svg

# With custom BPM rebase and lyrics
pjsekai-scores-rs master.sus --rebase rebase.json --lyric lyrics.txt -o master.svg

# SVG with custom CSS theme and local note assets
pjsekai-scores-rs master.sus --css dark.css --note-host /path/to/notes -o master.svg

# PNG via direct Skia rendering. Build with --features skia-image.
pjsekai-scores-rs master.sus --css dark.css --note-host /path/to/notes -o master.png

# JPEG via direct Skia rendering. JPEG quality defaults to 90.
pjsekai-scores-rs master.sus --css dark.css --note-host /path/to/notes --jpeg-quality 92 -o master.jpg

# Skia output with CSS-declared fonts. Build with --features skia-image.
pjsekai-scores-rs master.sus \
  --css black.css \
  --note-host /path/to/chart_asset/notes \
  --font-path /path/to/SourceHanSansSC-Regular.otf \
  --font-path /path/to/SourceHanSansSC-Bold.otf \
  --font-path /path/to/FOT-RodinNTLGPro-DB.otf \
  -o master.png

# A font directory can be used for ad-hoc runs; it is scanned recursively.
pjsekai-scores-rs master.sus --font-dir /path/to/fonts -o master.png --perf

# With Haruki/Saika chart request metadata
pjsekai-scores-rs master.sus \
  --note-host /path/to/chart_asset/notes \
  --jacket /path/to/jacket.png \
  --title "Tell Your World" \
  --artist kz \
  --difficulty master \
  --play-level 26 \
  --music-id 1 \
  -o master.svg
```

---

## Rust Crate Usage

Add to `Cargo.toml`:

```toml
[dependencies]
pjsekai-scores-rs = { path = "./pjsekai-scores-rs" }
```

### Basic example

```rust
use pjsekai_scores_rs::{Score, Drawing};

fn main() {
    let mut score = Score::open("master.sus").expect("failed to open score");
    score.meta.title = Some("Song Title".to_string());

    let mut drawing = Drawing::new(
        Some("https://asset3.pjsekai.moe/live/note/custom01".to_string()),
        None,   // extra CSS
        false,  // skill overlay
        None,   // music meta
        None,   // target segment seconds
        None,   // generator (default: "HarukiBot NEO")
    );

    let svg = drawing.svg(&mut score, None);
    std::fs::write("master.svg", svg).unwrap();
}
```

### With rebase (custom BPM)

```rust
use pjsekai_scores_rs::{Score, Rebase, Drawing};

let mut score = Score::open("master.sus").unwrap();
let rebase = Rebase::from_json(r#"{"musicId":1,"events":[{"bar":0,"bpm":160}]}"#).unwrap();
let mut rebased = rebase.apply(&mut score);

let mut drawing = Drawing::new(None, None, false, None, None, None);
let svg = drawing.svg(&mut rebased, None);
```

### Building (Rust only)

```bash
# Standard CLI: SVG output only
cargo build --release --bin pjsekai-scores-rs

# Skia CLI: SVG output + direct PNG/JPEG output
cargo build --release --features skia-image --bin pjsekai-scores-rs
```

GitHub CLI releases build both variants for each target. Skia-enabled assets are named with a `-skia-image` suffix.

---

## Python Wheel Usage

### Installation

```bash
# Default package: parser + SVG renderer.
pip install pjsekai-scores-rs

# Skia package: parser + SVG renderer + direct PNG/JPEG output.
pip install pjsekai-scores-rs-skia-image
```

or with uv:

```bash
# Default package: parser + SVG renderer.
uv add pjsekai-scores-rs

# Skia package: parser + SVG renderer + direct PNG/JPEG output.
uv add pjsekai-scores-rs-skia-image
```

Both packages expose the same Python module name:

```python
import pjsekai_scores_rs
```

Install one package or the other in an environment. The `pjsekai-scores-rs-skia-image` package is the optional image-output build; installing both packages at once is not supported because they provide the same extension module.

Or build and install from source (requires [maturin](https://github.com/PyO3/maturin)):

```bash
# Default wheel: Python bindings without Skia image output support.
maturin develop --release

# Optional Skia image wheel for local/private use.
maturin develop --release --features python,skia-image
```

### Python API

#### Score loading

`Score` is the shared parsed chart type used by SVG, Skia PNG/JPEG, timing APIs, and rebase. It accepts both `.sus` and Project SEKAI custom chart JSON.

```python
import pjsekai_scores_rs as scores

# Auto-detect by file extension/content: .sus or custom chart JSON.
score = scores.Score.open("master.sus")
score = scores.Score.open("chart.json")

# Force a format when needed.
score = scores.Score.open_sus("master.sus")
score = scores.Score.open_json("chart.json")

# Load from in-memory content.
score = scores.Score.from_str(sus_text)          # SUS only
score = scores.Score.from_json(json_text)        # JSON string or file-like object
score = scores.Score.from_dict(chart_dict)       # Python dict
score = scores.Score.load(sus_text_or_json_dict) # auto-detect string/file-like/dict
```

#### Score metadata and timing

```python
score.set_meta(
    title="Song Title",
    artist="Artist Name",
    difficulty="master",
    playlevel="30",
    jacket="file:///path/to/jacket.png",
    songid="1",
    subtitle="Optional subtitle",
)

score.title()        # -> str | None
score.artist()       # -> str | None
score.difficulty()   # -> str | None
score.playlevel()    # -> str | None
score.note_count()   # -> int
score.event_count()  # -> int

meta = score.meta
meta.title           # -> str | None
meta.subtitle        # -> str | None
meta.artist          # -> str | None
meta.genre           # -> str | None
meta.designer        # -> str | None
meta.difficulty      # -> str | None
meta.playlevel       # -> str | None
meta.songid          # -> str | None
meta.wave            # -> str | None
meta.waveoffset      # -> str | None
meta.jacket          # -> str | None
meta.background      # -> str | None
meta.movie           # -> str | None
meta.movieoffset     # -> float | None
meta.basebpm         # -> float | None

bar = scores.Fraction(4, 3)
time = score.get_time(bar)                    # -> Fraction
event = score.get_event(bar)                  # -> Event
delta = score.get_time_delta(0, bar)          # -> Fraction
bar_again = score.get_bar_by_time(float(time))

for event in score.events():
    event.bar              # -> Fraction
    event.bpm              # -> Fraction | None
    event.bar_length       # -> Fraction | None
    event.sentence_length  # -> int | None
    event.speed            # -> float | None
    event.section          # -> str | None
    event.text             # -> str | None
```

#### Fraction

`Fraction` is accepted by timing methods. Plain `int`, `float`, and fraction-like strings such as `"3/2"` are also accepted where a bar value is expected.

```python
bar = scores.Fraction(3, 2)
bar.numerator       # -> int
bar.denominator     # -> int
float(bar)          # -> 1.5
bar.limit_denominator(1000)
```

#### Rebase and lyric

```python
rebase = scores.Rebase.from_json('{"musicId":1,"events":[{"bar":0,"bpm":160}]}')
rebase = scores.Rebase.from_dict({"musicId": 1, "events": [{"bar": 0, "bpm": 160}]})
rebase = scores.Rebase.load(rebase_json_or_dict_or_file)

rebased_score = rebase.apply(score)
rebased_score = rebase.rebase(score)
rebased_score = rebase(score)

lyric = scores.Lyric.load(lyric_text_or_file)
lyric.word_count()  # -> int
```

#### Drawing

`Drawing` can render any `Score`, whether it came from SUS or JSON. `png()`, `jpg()`, and `jpeg()` require the `pjsekai-scores-rs-skia-image` package or a local build with `--features python,skia-image`.

```python
drawing = scores.Drawing(
    score=None,             # optional stored score
    lyric=None,             # optional stored lyric
    note_host="https://asset3.pjsekai.moe/live/note/custom01",
    style_sheet="",         # extra CSS appended after the built-in theme
    skill=False,            # render skill/fever overlays
    music_meta={            # optional, for skill overlay
        "fever_end_time": 45.0,
        "fever_score": 0.025,
        "skill_score_solo": [0.10, 0.15, 0.20, 0.25],
        "skill_score_multi": [0.05, 0.10, 0.15, 0.20],
    },
    generator="MyBot v1.0",
    note_asset_extension="png",
    font_paths=[
        "/path/to/SourceHanSansSC-Regular.otf",
        "/path/to/SourceHanSansSC-Bold.otf",
        "/path/to/FOT-RodinNTLGPro-DB.otf",
    ],
    font_dirs=None,
)

drawing.note_size = 18
drawing.time_height = 240.0
drawing.lane_width = 16

svg_string = drawing.svg(score)
svg_string = drawing.svg(score, lyric=lyric)

png_bytes = drawing.png(score)
jpg_bytes = drawing.jpg(score, jpeg_quality=90)
jpeg_bytes = drawing.jpeg(score, jpeg_quality=90)
```

`font_paths` and `font_dirs` only affect direct Skia PNG/JPEG rendering. SVG output keeps CSS as text and lets the viewer resolve fonts. For services, prefer explicit `font_paths` over broad `font_dirs`: directory inputs are scanned recursively before rendering, while loaded custom typefaces are cached per process by font path, modified time, and file size.

Skia image output parses CSS `font-family` declarations from the built-in theme plus `style_sheet`. It can match system fonts or custom fonts loaded from `font_paths` / `font_dirs`, including family names such as `Source Han Sans SC` and `FOT-RodinNTLG Pro DB`. When CJK glyphs are present, the renderer only uses a candidate typeface if it covers the required glyphs, then falls back to another matching custom/system font.

Custom font paths can also be changed after construction:

```python
drawing.set_font_paths(["/path/to/SourceHanSansSC-Regular.otf"])
drawing.add_font_path("/path/to/SourceHanSansSC-Bold.otf")
drawing.set_font_dirs(["/path/to/fonts"])
drawing.add_font_dir("/path/to/more-fonts")
```

#### Convenience functions

Use `score_to_*` for new code. The `sus_to_*` names are kept for compatibility and now also use the same auto-detecting score loader.

```python
svg = scores.score_to_svg(
    "master.sus",  # or "chart.json"
    note_host="https://asset3.pjsekai.moe/live/note/custom01",
    style_sheet="",
    rebase_json='{"musicId":1,"events":[{"bar":0,"bpm":160}]}',
    lyric_content=None,
    skill=False,
    music_meta=None,
    generator=None,
    note_asset_extension=None,
)

png = scores.score_to_png(
    "chart.json",
    note_host="/path/to/notes",
    style_sheet="",
    font_paths=[
        "/path/to/SourceHanSansSC-Regular.otf",
        "/path/to/FOT-RodinNTLGPro-DB.otf",
    ],
)

jpg = scores.score_to_jpg(
    "chart.json",
    note_host="/path/to/notes",
    style_sheet="",
    jpeg_quality=90,
)

jpeg = scores.score_to_jpeg("chart.json", note_host="/path/to/notes")

# Backward-compatible aliases.
svg = scores.sus_to_svg("master.sus")
png = scores.sus_to_png("master.sus", note_host="/path/to/notes")
jpg = scores.sus_to_jpg("master.sus", note_host="/path/to/notes")
jpeg = scores.sus_to_jpeg("master.sus", note_host="/path/to/notes")
```

#### Writing output files

```python
with open("master.svg", "w", encoding="utf-8") as f:
    f.write(svg_string)

with open("master.png", "wb") as f:
    f.write(png_bytes)

with open("master.jpg", "wb") as f:
    f.write(jpg_bytes)
```

---

## Building Wheels

### Current platform

```bash
# Default wheel: SVG renderer and parser.
maturin build --release

# Optional wheel with direct Skia PNG/JPEG rendering.
maturin build --release --features python,skia-image
```

The release workflow publishes that Skia-enabled build under the separate distribution name `pjsekai-scores-rs-skia-image` while keeping the import module as `pjsekai_scores_rs`.

### Cross-compile for Linux x64 (from macOS/Windows, requires [zig](https://ziglang.org))

```bash
# Python 3.14t
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.14 \
  maturin build --release --target x86_64-unknown-linux-gnu --zig -i python3.14t

# Python 3.13
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.13 \
  maturin build --release --target x86_64-unknown-linux-gnu --zig -i python3.13
```

Add `--features python,skia-image` to build an optional Skia image wheel from source.

### Cross-compile for Windows x64 (requires MinGW `x86_64-w64-mingw32-gcc`)

```bash
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.14 \
  maturin build --release --target x86_64-pc-windows-gnu -i python3.14t
```

Add `--features python,skia-image` to build an optional Skia image wheel from source.

---

## Project Structure

```
pjsekai-scores-rs/
├── Cargo.toml          # Rust package manifest + PyO3/Skia feature flags
├── pyproject.toml      # maturin build config (module name: pjsekai_scores_rs)
├── css/                # Built-in CSS themes (default, black, white, guess)
└── src/
    ├── main.rs         # CLI entry point (clap)
    ├── lib.rs          # Crate root + PyO3 module registration
    ├── fraction.rs     # Exact rational arithmetic (wraps num::Rational64)
    ├── meta.rs         # Score metadata (title, artist, difficulty, …)
    ├── line.rs         # .sus format line parser (LazyLock regexes, base36)
    ├── score.rs        # Score struct + 3-pass note linking + timing
    ├── score_json.rs   # Project SEKAI custom chart JSON parser
    ├── lyric.rs        # Lyric / Word structs + parser
    ├── rebase.rs       # BPM/timing rebase transformation
    ├── drawing.rs      # SVG renderer (direct String building)
    ├── skia_direct.rs  # Direct Skia PNG/JPEG renderer
    ├── python.rs       # PyO3 bindings (Score, Drawing, Rebase, Lyric, Event)
    ├── notes.rs        # NoteData enum + NoteBase + arena index pattern
    └── notes/
        ├── tap.rs          # TapType (8 variants)
        ├── directional.rs  # DirectionalType (6 variants)
        ├── slide.rs        # SlideType (4 variants) + Bézier path data
        └── event.rs        # Event struct (BPM / bar-length / speed / text)
```

## Notes

- The `python` feature gate enables PyO3. Without it, the crate builds as a pure Rust library + CLI binary with no Python dependency.
- `Score::open()` and `Score::parse_auto()` auto-detect JSON-looking custom chart input; use `Score::open_sus()` / `Score::parse()` or `Score::open_json()` / `Score::parse_json()` to force a format.
- The `skia-image` feature enables direct PNG/JPEG output. Python wheels omit it by default; build from source with `--features python,skia-image` when image bytes are needed.
- Skia image output parses CSS colors, font sizes, font weights, and `font-family`. Use `font_paths` / `font_dirs` or CLI `--font-path` / `--font-dir` when deployment fonts should not depend on the host system.
- `--perf` reports render, layout, setup, draw, encode, copy, write, and total timings. PNG encoding is lossless and can be much slower than JPEG on large charts.
- All `#[pyclass]` types are `Send + Sync` (no `Rc`/`RefCell`), satisfying Python 3.13t / 3.14t free-threaded requirements.
- CSS is embedded at compile time via `include_str!` — no runtime file lookup required.
- The `generate-import-lib` PyO3 feature is enabled so the Windows wheel can be cross-compiled without a local Windows Python installation.

## SVG rendering differences from Python original

The SVG output is functionally equivalent to the Python reference implementation, with the following intentional differences:

| Aspect | Python original | This implementation |
|---|---|---|
| `<use>` attribute | `xlink:href` (SVG 1.1) | `href` (SVG 2.0) |
| `<defs>` blocks | One per sub-SVG | Single merged block |
| Speed-change line layer | Below notes | **Above notes** (drawn last, for readability) |

The speed-change lines (purple horizontal lines marking BPM-speed events) are rendered on top of notes so they are not obscured when a note lands on the same row.

## License

MIT
