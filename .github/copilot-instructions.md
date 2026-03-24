# Copilot Instructions — pjsekai-scores-rs

## Project overview

Rust rewrite of the [pjsekai/scores](https://gitlab.com/pjsekai/scores) `.sus` parser and SVG chart renderer. Distributed as a Rust crate and a Python wheel (PyO3 0.28.2 / maturin).

**Do not modify `../scores/`** — it is the read-only reference Python implementation.

---

## Code style

- Use `rustfmt` defaults (no manual formatting rules).
- Prefer `impl From<X> for Y` over standalone conversion functions.
- Use `thiserror` for error types; propagate with `?`.
- Keep `python.rs` as a thin binding layer — no business logic. All logic lives in the core modules.
- Embed static assets (CSS) with `include_str!` at compile time.

---

## Architecture rules

### Notes use arena indexing
```rust
type NoteIdx = usize;
const NO_NOTE: NoteIdx = usize::MAX;
```
Cross-references between notes are stored as `NoteIdx` into `Score::notes: Vec<NoteData>`. Never introduce `Rc`, `Arc`, or `RefCell` — they break PyO3 free-threaded compatibility.

### `#[cfg(feature = "python")]` guards all PyO3 code
The crate must build as a pure Rust library without the `python` feature:
```bash
cargo check              # pure Rust
cargo check --features python  # with PyO3
```

### `pub init_notes()` and `pub init_events()` on Score
Called by `rebase.rs` after rebuilding note/event vectors. Keep them `pub`.

### Borrow checker in drawing.rs
`self.build_skill_covers(score)` is a `&mut self` call. Acquire `&self.config` **after** it:
```rust
// ✅ correct
self.build_skill_covers(score);
let cfg = &self.config;

// ❌ compile error (E0502)
let cfg = &self.config;
self.build_skill_covers(score);
```

### Raw strings containing `href="#`
Use `r##"..."##`, not `r#"..."#`:
```rust
format!(r##"<use href="#{id}"/>"##, id = id)  // ✅
format!(r#"<use href="#{id}"/>"#, id = id)    // ❌ syntax error
```

---

## Python API conventions

Public Python-facing names use snake_case matching the original `pjsekai.scores` API where possible. Key differences from the Python original that must be preserved:

- `Score.set_meta(**kwargs)` (not attribute assignment)
- `Rebase.from_dict(d).apply(score)` (not `load_from_dict` / `rebase`)
- `Drawing.svg(score)` returns `str` (not `svgwrite.Drawing`)
- `Lyric.load(string)` (not file object)
- `score.events()` is a method (not attribute)

---

## Build & test

```bash
cargo build --release                   # Rust crate + CLI
cargo test                              # Rust unit tests
maturin build --release -i python3.14t  # Python 3.14t wheel
uv pip install target/wheels/*.whl      # Install into venv
```

Benchmarking (measured 2026-03-24):  
**Python original: 1.879s → Rust: 0.020s → 95.4× speedup**  
Environment: Debian 12 · Intel Xeon Platinum 8272CL × 8 cores @ 2.594 GHz · Python 3.13 · AMD64 · both pipelines running concurrently
