# AGENTS.md — pjsekai-scores-rs

Guidance for AI coding agents working on this codebase.

## What this project is

A Rust rewrite of [pjsekai/scores](https://gitlab.com/pjsekai/scores) — a `.sus` / Project SEKAI custom chart parser, SVG chart renderer, and direct Skia PNG/JPEG renderer. It ships as a **Rust crate**, **CLI**, and **Python extension wheel** (PyO3/maturin).

The original Python implementation lives at `../scores/` and is the reference for correctness. Do not modify it.

---

## Build commands

```bash
# Rust only (crate + CLI)
cargo build --release
cargo build --release --features skia-image
cargo check
cargo check --features 'python skia-image'
cargo check --target wasm32-unknown-unknown --no-default-features --features wasm --lib
cargo test --features skia-image

# Python wheel (current platform, active venv)
maturin build --release
maturin build --release --features python,skia-image
# or for development install:
maturin develop --release
maturin develop --release --features python,skia-image

# WebAssembly (SVG only; no skia-image)
wasm-pack build --release --target web --no-default-features --features wasm

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
├── skia_direct.rs  Direct Skia PNG/JPEG renderer + CSS/font handling
├── python.rs       All PyO3 bindings (PyScore, PyDrawing, PyRebase, PyLyric, PyEvent)
├── wasm.rs         wasm-bindgen bindings (Score, Drawing, Rebase, Lyric; SVG only)
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

### WebAssembly API
The `wasm` feature is independent from `python` and `skia-image`. It exposes `Score`, `Drawing`, `Rebase`, and `Lyric` through `wasm-bindgen` for in-memory `.sus` / custom-chart JSON parsing and SVG string output. Keep browser-facing APIs content-based (`Score.fromSus`, `Score.fromJson`, `Score.load`, `Drawing.svg`) instead of file-path based; `Score::open*`, CLI code, local font scanning, and Skia raster output are not part of the wasm surface.

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

### Skia custom fonts and CSS `font-family`
Skia image output honors CSS `font-family`, `font-weight`, and `font-size` parsed from the built-in theme plus runtime `style_sheet`. The SVG renderer only emits CSS; custom font loading is for direct PNG/JPEG rendering.

Custom fonts enter through `DrawingConfig.font_paths` / `font_dirs`, CLI `--font-path` / `--font-dir`, and Python `Drawing(..., font_paths=..., font_dirs=...)` plus setter methods. `font_dirs` are scanned recursively for `.ttf`, `.otf`, and `.ttc`; prefer explicit `font_paths` in services to avoid scanning large asset roots on hot paths.

`skia_direct.rs` registers custom typefaces by localized family name, Skia family name, and PostScript name, all normalized for lookup. This lets CSS names such as `FOT-RodinNTLG Pro DB` and `Source Han Sans SC` match bundled fonts. When CJK text is present, a candidate typeface must cover the required glyphs before it is selected.

Custom font data is cached per process in `CUSTOM_FONT_CACHE`, keyed by sorted font path, modified time, and file size. Keep that key stable when changing font loading; stale font cache bugs are harder to diagnose than a small setup cost.

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
| system fonts only for raster text | `font_paths` / `font_dirs` for Skia PNG/JPEG |

---

## Things to avoid

- **Do not add `Rc` or `RefCell`** — breaks free-threaded Python compatibility.
- **Do not modify `../scores/`** — it is the reference Python implementation.
- **Do not call `maturin develop` with the 3.14t venv** — it fails; use `maturin build -i python3.14t` then `uv pip install`.
- **Do not use `r#"..."#`** for strings that embed `href="#` — use `r##"..."##`.
- **Do not move `let cfg = &self.config`** before the `build_skill_covers()` call in `drawing.rs`.
- **Do not rename `notes.rs` back to `notes/mod.rs`** — the module root lives at `src/notes.rs`; submodules stay in `src/notes/`.
- **Do not rely on host-installed fonts for deployed Skia output** — expose/pass font files or directories instead.
- **Do not point `font_dirs` at broad asset roots in performance-sensitive services** unless that scan cost is acceptable. Prefer known `font_paths`.

---

## Release and verification notes

- Release commits and release tags must be GPG-signed. Verify with `git log -1 --show-signature` and `git tag -v vX.Y.Z`.
- A GitHub Release/tag triggers three release workflows: Crate, CLI, and Python. Check all three, especially the `pjsekai-scores-rs-skia-image` Python publish job.
- For Skia/font/API changes, run `cargo check --features 'python skia-image'` and `cargo test --features skia-image` before release.
- When changing CLI/Python options, update `README.md`, `AGENTS.md`, and `CLAUDE.md` in the same docs pass.

---

## Git commits

All commit subjects must follow:

```text
[Type] Short description starting with capital letter
```

Allowed types:

| Type      | Usage                                                 |
|-----------|-------------------------------------------------------|
| `[Feat]`  | New feature or capability                             |
| `[Fix]`   | Bug fix                                               |
| `[Chore]` | Maintenance, refactoring, dependency or build changes |
| `[Docs]`  | Documentation-only changes                            |

Rules:

- Description starts with a capital letter.
- Use imperative mood: `Add ...`, not `Added ...`.
- No trailing period.
- Keep the subject at or below roughly 70 characters.
- **Agent attribution uses the standard Git `Co-authored-by:` trailer in the commit body, not a free-form `Agent:` line.** This makes GitHub render the co-author avatar on the commit page. The trailer must be on its own line, separated from the subject by a blank line, in the form `Co-authored-by: <Display Name> <email>`. Suggested values per agent:
  - Claude (any 4.x): `Co-authored-by: Claude Opus 4.7 <noreply@anthropic.com>` (substitute the actual model, e.g. `Claude Sonnet 4.6`, `Claude Haiku 4.5`)
  - Codex: `Co-authored-by: Codex <noreply@openai.com>`
  - Copilot: `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>`

Examples from this repo's history:

```text
[Feat] Add wasm bindings and Python type stubs
[Fix] Collapse JSON slide link condition
[Chore] Update dependencies
[Docs] Update Skia font documentation
```

## GitHub Actions workflows

Use the standardized workflow layout in `.github/workflows`:

- `ci.yml` runs on `main` pushes, pull requests targeting `main`, and manual dispatch.
- Rust CI order: `cargo fmt --all -- --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, then `cargo test --locked`.
- `release.yml` is the standard release build entrypoint. It runs on `v*` tags and manual dispatch, builds release artifacts, uploads them with `actions/upload-artifact`, and publishes GitHub Release assets on tag pushes.
- `release-crate.yml` publishes the Rust crate and keeps its package-specific release flow.
- `release-python.yml` builds and publishes Python artifacts and keeps its package-specific release flow.

Workflow maintenance rules:

- Keep workflow filenames and top-level names aligned: `CI`, `Release`, `Docker`, and optional package-specific names.
- Use `actions/checkout@v6`, `actions/setup-go@v6`, `actions/upload-artifact@v7`, `actions/download-artifact@v8`, `softprops/action-gh-release@v3`, and current Docker actions (`setup-buildx@v4`, `login@v4`, `metadata@v6`, `build-push@v7`).
- Keep `permissions` minimal: `contents: read` for CI/Docker build-only work, `contents: write` for release publishing, and `packages: write` only when pushing container images.
- Use workflow `concurrency` keyed by workflow name and ref, with release jobs using `release-${{ github.ref_name }}` and `cancel-in-progress: false`.
- Do not reintroduce legacy workflow names such as `rust-ci.yml`, `build.yml`, `release-build.yml`, `docker-build.yml`, or `docker-release.yml` unless a package-specific workflow already exists and is intentionally preserved.
