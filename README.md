# pjsekai-scores-rs

Project SEKAI score (`.sus`) parser and SVG chart renderer, rewritten in Rust.

- Original Python project: [pjsekai/scores](https://gitlab.com/pjsekai/scores)
- Skill information previewer based on [xfl03's fork](https://github.com/xfl03/SekaiMusicChart)

## Features

- Parses `.sus` format score files (Score format specification v2.7 rev2)
- Generates SVG chart images with full note rendering (Tap / Directional / Slide)
- BPM rebase support (custom timing via JSON)
- Lyric overlay support
- Skill/Fever cover overlay support
- Dual distribution: **Rust crate** and **Python wheel** (via PyO3)
- Python 3.13 / 3.14t (free-threaded) compatible

## Performance

Measured in production against the original Python implementation, both running concurrently on the same request:

| Metric | Python | Rust | Speedup |
|---|---|---|---|
| Full pipeline (parse → rebase → SVG) | 1.879s | 0.020s | **95.4×** |

> **Environment:** Debian 12 · Intel Xeon Platinum 8272CL × 8 cores @ 2.594 GHz · Python 3.13 · AMD64

Phase-level breakdown:

| Phase | Estimated Speedup |
|---|---|
| `.sus` parsing | ~8–10× |
| Note linking (3-pass) | ~10–12× |
| SVG generation | ~30–40× |

The dominant win is SVG generation: Rust replaces thousands of Python `svgwrite` object allocations and DOM traversals with direct `String` building and pre-computed layout arithmetic.

---

## CLI Usage

```
pjsekai-scores <SCORE> [OPTIONS]

Arguments:
  <SCORE>  The .sus score file

Options:
      --rebase <REBASE>          Customized BPM, beats and sections (JSON)
      --lyric <LYRIC>            Lyrics file
      --css <CSS>                Custom CSS stylesheet
      --note-host <NOTE_HOST>    Base URL for note asset files
                                 [default: https://asset3.pjsekai.moe/live/note/custom01]
      --generator <GENERATOR>    Generator name shown in the SVG subtitle
                                 [default: HarukiBot NEO]
  -o, --output <OUTPUT>          Output SVG file path
  -h, --help                     Print help
```

### Example

```bash
# Basic conversion
pjsekai-scores master.sus -o master.svg

# With custom BPM rebase and lyrics
pjsekai-scores master.sus --rebase rebase.json --lyric lyrics.txt -o master.svg

# With custom CSS theme and local note assets
pjsekai-scores master.sus --css dark.css --note-host /path/to/notes -o master.svg
```

---

## Rust Crate Usage

Add to `Cargo.toml`:

```toml
[dependencies]
pjsekai-scores = { path = "./pjsekai-scores-rs" }
```

### Basic example

```rust
use pjsekai_scores::{Score, Drawing};

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
use pjsekai_scores::{Score, Rebase, Drawing};

let mut score = Score::open("master.sus").unwrap();
let rebase = Rebase::from_json(r#"{"musicId":1,"events":[{"bar":0,"bpm":160}]}"#).unwrap();
let mut rebased = rebase.apply(&mut score);

let mut drawing = Drawing::new(None, None, false, None, None, None);
let svg = drawing.svg(&mut rebased, None);
```

### Building (Rust only)

```bash
cargo build --release
```

---

## Python Wheel Usage

### Installation

```bash
pip install pjsekai_scores_rs-0.1.0-*.whl
```

Or build and install from source (requires [maturin](https://github.com/PyO3/maturin)):

```bash
maturin develop --release
```

### Available wheels

| File | Platform |
|---|---|
| `cp314-cp314t-macosx_11_0_arm64.whl` | macOS ARM64 / Python 3.14t |
| `cp313-cp313-macosx_11_0_arm64.whl` | macOS ARM64 / Python 3.13 |
| `cp314-cp314t-manylinux_2_17_x86_64.whl` | Linux x64 / Python 3.14t |
| `cp313-cp313-manylinux_2_17_x86_64.whl` | Linux x64 / Python 3.13 |
| `cp314-cp314t-win_amd64.whl` | Windows x64 / Python 3.14t |

### Python API

```python
import pjsekai_scores

# ── Score ─────────────────────────────────────────────────────────────────────
score = pjsekai_scores.Score.open("master.sus")          # load from file
score = pjsekai_scores.Score.from_str(sus_text)          # load from string

score.set_meta(
    title="Song Title",
    artist="Artist Name",
    difficulty="master",
    playlevel="30",
    jacket="file:///path/to/jacket.png",
    songid="1",
)

score.title()        # -> Optional[str]
score.artist()       # -> Optional[str]
score.difficulty()   # -> Optional[str]
score.playlevel()    # -> Optional[str]
score.note_count()   # -> int
score.event_count()  # -> int
score.events()       # -> List[Event]  (each has .bar, .bpm, .speed, .text)

# ── Rebase ────────────────────────────────────────────────────────────────────
rebase = pjsekai_scores.Rebase.from_json('{"musicId":1,"events":[{"bar":0,"bpm":160}]}')
rebase = pjsekai_scores.Rebase.from_dict({"musicId": 1, "events": [{"bar": 0, "bpm": 160}]})
rebased_score = rebase.apply(score)

# ── Lyric ─────────────────────────────────────────────────────────────────────
lyric = pjsekai_scores.Lyric.load(lyric_text)   # load from string
lyric.word_count()  # -> int

# ── Drawing ───────────────────────────────────────────────────────────────────
drawing = pjsekai_scores.Drawing(
    note_host="https://asset3.pjsekai.moe/live/note/custom01",
    style_sheet="",         # extra CSS appended after the built-in theme
    skill=False,            # render skill/fever overlays
    music_meta={            # optional, for skill overlay
        "fever_end_time": 45.0,
        "fever_score": 0.025,
        "skill_score_solo": [0.10, 0.15, 0.20, 0.25],
        "skill_score_multi": [0.05, 0.10, 0.15, 0.20],
    },
    target_segment_seconds=8.0,  # approximate seconds per chart column
    generator="MyBot v1.0",      # optional: overrides the default "HarukiBot NEO"
)

# Configurable properties
drawing.note_size = 18      # note sprite size in pixels (default 16)
drawing.time_height = 240.0 # pixels per second (default 360.0)
drawing.lane_width = 16     # lane width in pixels (default 16)

svg_string = drawing.svg(score)              # returns str
svg_string = drawing.svg(score, lyric=lyric) # with lyrics

# Write to file
with open("master.svg", "w") as f:
    f.write(svg_string)

# ── Convenience function ──────────────────────────────────────────────────────
svg = pjsekai_scores.sus_to_svg(
    "master.sus",
    note_host="https://asset3.pjsekai.moe/live/note/custom01",
    style_sheet="",
    rebase_json='{"musicId":1,"events":[{"bar":0,"bpm":160}]}',
    lyric_content=None,
    skill=False,
    music_meta=None,
    target_segment_seconds=8.0,
    generator=None,  # optional: overrides the default "HarukiBot NEO"
)
```

---

## Building Wheels

### Current platform

```bash
maturin build --release
```

### Cross-compile for Linux x64 (from macOS/Windows, requires [zig](https://ziglang.org))

```bash
# Python 3.14t
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.14 \
  maturin build --release --target x86_64-unknown-linux-gnu --zig -i python3.14t

# Python 3.13
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.13 \
  maturin build --release --target x86_64-unknown-linux-gnu --zig -i python3.13
```

### Cross-compile for Windows x64 (requires MinGW `x86_64-w64-mingw32-gcc`)

```bash
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.14 \
  maturin build --release --target x86_64-pc-windows-gnu -i python3.14t
```

---

## Project Structure

```
pjsekai-scores-rs/
├── Cargo.toml          # Rust package manifest + PyO3 feature flag
├── pyproject.toml      # maturin build config (module name: pjsekai_scores)
├── css/                # Built-in CSS themes (default, black, white, guess)
└── src/
    ├── main.rs         # CLI entry point (clap)
    ├── lib.rs          # Crate root + PyO3 module registration
    ├── fraction.rs     # Exact rational arithmetic (wraps num::Rational64)
    ├── meta.rs         # Score metadata (title, artist, difficulty, …)
    ├── line.rs         # .sus format line parser (LazyLock regexes, base36)
    ├── score.rs        # Score struct + 3-pass note linking + timing
    ├── lyric.rs        # Lyric / Word structs + parser
    ├── rebase.rs       # BPM/timing rebase transformation
    ├── drawing.rs      # SVG renderer (~750 lines, direct String building)
    ├── python.rs       # PyO3 bindings (Score, Drawing, Rebase, Lyric, Event)
    └── notes/
        ├── mod.rs      # NoteData enum + NoteBase + arena index pattern
        ├── tap.rs      # TapType (8 variants)
        ├── directional.rs  # DirectionalType (6 variants)
        ├── slide.rs    # SlideType (4 variants) + Bézier path data
        └── event.rs    # Event struct (BPM / bar-length / speed / text)
```

## Notes

- The `python` feature gate enables PyO3. Without it, the crate builds as a pure Rust library + CLI binary with no Python dependency.
- All `#[pyclass]` types are `Send + Sync` (no `Rc`/`RefCell`), satisfying Python 3.13t / 3.14t free-threaded requirements.
- CSS is embedded at compile time via `include_str!` — no runtime file lookup required.
- The `generate-import-lib` PyO3 feature is enabled so the Windows wheel can be cross-compiled without a local Windows Python installation.

## License

MIT
