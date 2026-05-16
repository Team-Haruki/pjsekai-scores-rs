# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Rust rewrite of [pjsekai/scores](https://gitlab.com/pjsekai/scores) — a `.sus` format score parser and SVG chart renderer for Project SEKAI. Ships as both a Rust crate/CLI and a Python extension wheel (via PyO3/maturin). The original Python implementation at `../scores/` is the reference for correctness — do not modify it.

## Build and test commands

```bash
cargo build --release                         # Standard CLI binary
cargo build --release --features skia-image   # Skia image CLI binary
cargo check                                   # Type check (pure Rust, no PyO3)
cargo check --features python                 # Type check with PyO3 bindings
cargo check --features 'python skia-image'    # Type check optional image bindings
cargo test                                    # Run all tests
cargo test <test_name>                        # Run a single test
cargo clippy -- -D warnings                   # Lint (CI requires clean)
cargo fmt --all --check                       # Format check (CI requires clean)
cargo fmt                                     # Auto-format

maturin build --release                       # Default Python wheel
maturin build --release --features python,skia-image  # Optional Skia image wheel
maturin develop --release                     # Build + install default wheel into active venv
```

CI runs `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --all --check` on every push/PR to main.

## Architecture

### Dual distribution
The `python` feature gate (`#[cfg(feature = "python")]`) enables PyO3 bindings in `python.rs`. Without it, the crate builds as a pure Rust library + CLI. The `pyproject.toml` / maturin config automatically enables this feature for wheel builds. The `skia-image` feature is optional for Python wheels and must be requested explicitly when building Skia-enabled wheels.

### Arena pattern for notes
Notes live in `Score::notes: Vec<NoteData>`. Cross-references (slide head/tail/next) use `NoteIdx = usize` with `NO_NOTE = usize::MAX`. Never use `Rc`, `Arc`, or `RefCell` — they break free-threaded Python (3.13t/3.14t) compatibility.

### 3-pass note linking (`score.rs`)
1. Parse `.sus` lines into a flat `Vec<NoteData>`
2. Group slide notes by channel; link head → body → tail chains
3. Associate tap/directional notes with their adjacent slides

### SVG rendering (`drawing.rs`)
Direct `String` building via `std::fmt::Write` — no DOM library. CSS themes are embedded at compile time with `include_str!`. Borrow order matters: `self.build_skill_covers(score)` (`&mut self`) must be called **before** taking `&self.config`.

### `python.rs` is a thin binding layer
All business logic lives in the core modules. `python.rs` only wraps types for PyO3.

### `pub init_notes()` / `pub init_events()`
These must remain `pub` — called by `rebase.rs` after rebuilding note/event vectors.

### `ParsedItem::Meta` is boxed
`ParsedItem::Meta(Box<Meta>)` to avoid a `large_enum_variant` clippy warning.

### Raw strings with `href="#`
Use `r##"..."##` (not `r#"..."#`) for format strings containing `href="#` — the `"#` sequence prematurely closes single-hash raw strings.

## Git commit format

All commits **must** follow `[Type] Short description` where Type is one of `[Feat]`, `[Fix]`, `[Chore]`, `[Docs]`. Description starts with a capital letter, imperative mood, no trailing period, ≤ ~70 chars. Agent commits must include a `Co-Authored-By` trailer identifying the agent.
