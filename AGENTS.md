# AGENTS.md — pjsekai-scores-rs

Guidance for AI coding agents working on this codebase.

## What this project is

A Rust rewrite of [pjsekai/scores](https://gitlab.com/pjsekai/scores) — a `.sus` format score parser and SVG chart renderer for Project SEKAI. It ships as both a **Rust crate** and a **Python extension wheel** (PyO3/maturin).

The original Python implementation lives at `../scores/` and is the reference for correctness. Do not modify it.

---

## Build commands

```bash
# Rust only (crate + CLI)
cargo build --release
cargo build --release --features skia-image
cargo check

# Python wheel (current platform, active venv)
maturin build --release
maturin build --release --features python,skia-image
# or for development install:
maturin develop --release
maturin develop --release --features python,skia-image

# Python 3.14t free-threaded wheel (macOS ARM64)
maturin build --release -i python3.14t

# Cross-compile for Linux x64
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.14 \
  maturin build --release --target x86_64-unknown-linux-gnu --zig -i python3.14t

# Cross-compile for Windows x64
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.14 \
  maturin build --release --target x86_64-pc-windows-gnu -i python3.14t
```

---

## Project layout

```
src/
├── main.rs         CLI entry point (clap); binary name: pjsekai-scores-rs
├── lib.rs          Crate root; registers PyO3 module under `python` feature
├── fraction.rs     Exact rational arithmetic (num::Rational64 wrapper)
├── meta.rs         Score metadata struct
├── line.rs         .sus format line parser (LazyLock regexes, base36 decode)
├── score.rs        Score + 3-pass note-linking + get_time() timing engine
├── lyric.rs        Lyric/Word parser
├── rebase.rs       BPM/timing rebase transformation
├── drawing.rs      SVG renderer — direct String building, ~750 lines
├── python.rs       All PyO3 bindings (PyScore, PyDrawing, PyRebase, PyLyric, PyEvent)
├── notes.rs        NoteData enum, arena index pattern (NoteIdx = usize)
└── notes/
    ├── tap.rs          TapType (8 variants)
    ├── directional.rs  DirectionalType (6 variants)
    ├── slide.rs        SlideType + Bézier path data
    └── event.rs        Event (BPM / bar-length / speed / text)
```

---

## Key architectural decisions

### `Score::parse` and `impl std::str::FromStr`
`Score` implements `std::str::FromStr`. Use `Score::parse(content)` as the public Rust method, or `content.parse::<Score>()` via the trait. The Python binding `Score.from_str(s)` delegates to `s.parse::<Score>().unwrap()`.

### `DrawingConfig.generator` / `Drawing::new` signature
`DrawingConfig` carries a `generator: String` field (default `"HarukiBot NEO"`). `Drawing::new` accepts `generator: Option<String>` as the 6th argument — `None` keeps the default. The SVG subtitle reads from this field. Python exposes it as a `generator=None` keyword argument on `Drawing(...)` and `sus_to_svg(...)`.

### `ParsedItem::Meta` is boxed
`ParsedItem::Meta(Box<Meta>)` — the variant wraps `Box<Meta>` to avoid a large-enum-variant clippy warning. Call sites use `self.meta.merge(&m)` unchanged because `Box<Meta>` auto-derefs.

### Arena pattern for notes
Notes are stored in `Vec<NoteData>` on `Score`. Cross-references (slide head/tail/next) use `NoteIdx = usize` with `NO_NOTE = usize::MAX`. This avoids Rust's circular reference restrictions without `Rc`/`RefCell`.

### 3-pass note linking (score.rs)
1. Parse all raw `.sus` lines → flat `Vec<NoteData>`
2. Group slide notes by channel; link `head → body → tail` chains
3. Link tap-like notes to their adjacent tick events

### `pub init_notes()` / `pub init_events()`
These are called by `rebase.rs` after rebuilding note/event vectors. They must remain `pub`.

### `pub timed_events_cache`
Accessed directly by `rebase.rs` to pre-compute bar→time mappings without borrowing conflicts.

### Borrow checker in rebase.rs
`source.active_notes` iteration and `source.get_time()` (mutably populates cache) cannot coexist. Fix: clone `active_notes` and `notes` snapshots first, pre-compute all bar→time values into a `HashMap`, then iterate the snapshot.

### Drawing.svg() borrow order
`self.build_skill_covers(score)` takes `&mut self`. The `let cfg = &self.config` binding must come **after** this mutable call, not before. Violating this causes E0502.

### Raw strings with `href="#`
The literal `href="#` contains `"#` which prematurely closes `r#"..."#` raw strings. Use `r##"..."##` for any format string containing this pattern.

---

## Python bindings (python.rs)

### Feature gate
All PyO3 code is behind `#[cfg(feature = "python")]`. The crate builds as a pure Rust library + CLI without it.

### Free-threaded Python (3.13t / 3.14t)
All `#[pyclass]` types own their data (no `Rc`/`RefCell`) so they are `Send + Sync` automatically. PyO3 0.28.2 supports free-threaded Python natively.

### Windows cross-compilation
The `generate-import-lib` PyO3 feature generates a Python import `.lib` at build time, removing the need for a Windows Python installation when cross-compiling.

### API differences from original Python
| Python (`pjsekai.scores`) | Rust (`pjsekai_scores_rs`) |
|---|---|
| `Drawing(score=score)` + `drawing.svg().saveas(path)` | `Drawing(...)` + `drawing.svg(score)` → `str` |
| `score.meta.xxx = val` | `score.set_meta(xxx=val)` |
| `Rebase.load_from_dict(d).rebase(score)` | `Rebase.from_dict(d).apply(score)` |
| `Lyric.load(file_obj)` | `Lyric.load(string)` |
| `score.events` (attribute) | `score.events()` (method) |
| *(no generator param)* | `Drawing(generator="…")` / `sus_to_svg(generator="…")` |

---

## Things to avoid

- **Do not add `Rc` or `RefCell`** — breaks free-threaded Python compatibility.
- **Do not modify `../scores/`** — it is the reference Python implementation.
- **Do not call `maturin develop` with the 3.14t venv** — it fails; use `maturin build -i python3.14t` then `uv pip install`.
- **Do not use `r#"..."#`** for strings that embed `href="#` — use `r##"..."##`.
- **Do not move `let cfg = &self.config`** before the `build_skill_covers()` call in `drawing.rs`.
- **Do not rename `notes.rs` back to `notes/mod.rs`** — the module root lives at `src/notes.rs`; submodules stay in `src/notes/`.

---

## Git commit format

All commits **must** follow:

```
[Type] Short description starting with capital letter
```

| Type      | Usage                                                 |
|-----------|-------------------------------------------------------|
| `[Feat]`  | New feature or capability                             |
| `[Fix]`   | Bug fix                                               |
| `[Chore]` | Maintenance, refactoring, dependency or build changes |
| `[Docs]`  | Documentation-only changes                            |

Rules:

- Description starts with a **capital letter**.
- Imperative mood (`Add ...`, not `Added ...`).
- No trailing period.
- Keep subject ≤ ~70 chars.
- **Agent commits must include a `Co-Authored-By` trailer** identifying the agent.

Examples:

```
[Feat] Add toolbox live snapshot provider
[Fix] Move user_snapshot config under pjsk_render
[Chore] Rename config file to haruki-cloud.yaml
[Docs] Update known-bugs.md with snapshot fix
```
