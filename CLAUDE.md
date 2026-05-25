# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Rust rewrite of [pjsekai/scores](https://gitlab.com/pjsekai/scores) — a `.sus` / Project SEKAI custom chart parser, SVG chart renderer, and direct Skia PNG/JPEG renderer. Ships as a Rust crate/CLI and a Python extension wheel (via PyO3/maturin). The original Python implementation at `../scores/` is the reference for correctness — do not modify it.

## Build and test commands

```bash
cargo build --release                         # Standard CLI binary
cargo build --release --features skia-image   # Skia image CLI binary
cargo check                                   # Type check (pure Rust, no PyO3)
cargo check --features python                 # Type check with PyO3 bindings
cargo check --features 'python skia-image'    # Type check optional image bindings
cargo check --target wasm32-unknown-unknown --no-default-features --features wasm --lib  # Check wasm bindings
cargo test                                    # Run all tests
cargo test --features skia-image              # Run tests including Skia/CSS/font code
cargo test <test_name>                        # Run a single test
cargo clippy -- -D warnings                   # Lint (CI requires clean)
cargo fmt --all --check                       # Format check (CI requires clean)
cargo fmt                                     # Auto-format

maturin build --release                       # Default Python wheel
maturin build --release --features python,skia-image  # Optional Skia image wheel
maturin develop --release                     # Build + install default wheel into active venv

wasm-pack build --release --target web --no-default-features --features wasm  # SVG-only wasm package
```

CI runs `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --all --check` on every push/PR to main.

## Architecture

### Dual distribution
The `python` feature gate (`#[cfg(feature = "python")]`) enables PyO3 bindings in `python.rs`. Without it, the crate builds as a pure Rust library + CLI. The `pyproject.toml` / maturin config automatically enables this feature for wheel builds. The `skia-image` feature is optional for Python wheels and must be requested explicitly when building Skia-enabled wheels.

### WebAssembly bindings
The `wasm` feature enables `wasm-bindgen` exports in `wasm.rs`. This surface is content-based and SVG-only: use `Score.fromSus`, `Score.fromJson`, `Score.load`, `Rebase.fromJson`, `Lyric.fromText`, and `Drawing.svg` from browser or worker code. Do not add file-path based APIs, Skia raster output, or local font scanning to the wasm surface.

### Arena pattern for notes
Notes live in `Score::notes: Vec<NoteData>`. Cross-references (slide head/tail/next) use `NoteIdx = usize` with `NO_NOTE = usize::MAX`. Never use `Rc`, `Arc`, or `RefCell` — they break free-threaded Python (3.13t/3.14t) compatibility.

### 3-pass note linking (`score.rs`)
1. Parse `.sus` lines into a flat `Vec<NoteData>`
2. Group slide notes by channel; link head → body → tail chains
3. Associate tap/directional notes with their adjacent slides

### SVG rendering (`drawing.rs`)
Direct `String` building via `std::fmt::Write` — no DOM library. CSS themes are embedded at compile time with `include_str!`. Borrow order matters: `self.build_skill_covers(score)` (`&mut self`) must be called **before** taking `&self.config`.

### Skia image rendering (`skia_direct.rs`)
Direct PNG/JPEG output is behind the `skia-image` feature. It parses CSS colors, `font-size`, `font-weight`, and `font-family` from the built-in theme plus runtime `style_sheet`. CLI `--font-path` / `--font-dir` and Python `font_paths` / `font_dirs` load custom `.ttf`, `.otf`, and `.ttc` fonts for Skia output only; SVG output still leaves font resolution to the viewer.

Prefer explicit font files in services. `font_dirs` are recursive and can be expensive when pointed at broad asset roots. Custom typefaces are cached per process by sorted font path, modified time, and file size, so repeated Python API renders should not reread the same CJK/JP fonts.

CSS family lookup uses localized family names, Skia family names, and PostScript names after normalization. This is why names like `Source Han Sans SC` and `FOT-RodinNTLG Pro DB` can work when those font files are passed in. For CJK text, candidate typefaces must cover the required glyphs before selection.

### `python.rs` is a thin binding layer
All business logic lives in the core modules. `python.rs` only wraps types for PyO3. Keep `font_paths` / `font_dirs` available on `Drawing`, `score_to_svg/png/jpg/jpeg`, and the backward-compatible `sus_to_*` helpers when adjusting rendering arguments.

### `wasm.rs` is a thin binding layer
Keep wasm-only glue in `wasm.rs` and route behavior through the core `Score`, `Drawing`, `Rebase`, and `Lyric` types. The wasm API should stay independent from the `python` and `skia-image` features.

### `pub init_notes()` / `pub init_events()`
These must remain `pub` — called by `rebase.rs` after rebuilding note/event vectors.

### `ParsedItem::Meta` is boxed
`ParsedItem::Meta(Box<Meta>)` to avoid a `large_enum_variant` clippy warning.

### Raw strings with `href="#`
Use `r##"..."##` (not `r#"..."#`) for format strings containing `href="#` — the `"#` sequence prematurely closes single-hash raw strings.

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
