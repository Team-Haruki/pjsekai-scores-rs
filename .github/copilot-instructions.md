# Copilot Instructions — pjsekai-scores-rs

## Project overview

Rust rewrite of the [pjsekai/scores](https://gitlab.com/pjsekai/scores) `.sus` parser, Project SEKAI custom chart JSON parser, and SVG chart renderer. Distributed as a Rust crate (`pjsekai-scores-rs`) and Python wheels via PyO3 0.28.2 / maturin. The default PyPI package is `pjsekai-scores-rs`; the optional Skia image-output package is `pjsekai-scores-rs-skia-image`. Both import as `pjsekai_scores_rs`.

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

### `Score::parse` / `impl std::str::FromStr`
`Score` implements `std::str::FromStr`. Rust callers use `Score::parse(s)` or `s.parse::<Score>()`. The Python binding `Score.from_str(s)` delegates to `s.parse::<Score>().unwrap()`.

### `DrawingConfig.generator` / `Drawing::new` signature
`DrawingConfig` has a `generator: String` field (default `"HarukiBot NEO"`). `Drawing::new` takes `generator: Option<String>` as the 6th argument; `None` keeps the default. Python exposes it as `generator=None` on `Drawing(...)` and `sus_to_svg(...)`.

### `notes.rs` module root
The notes module root is `src/notes.rs` (not `src/notes/mod.rs`). Submodules `tap`, `directional`, `slide`, `event` remain in `src/notes/`.

### `ParsedItem::Meta` is boxed
`ParsedItem::Meta(Box<Meta>)` avoids a `large_enum_variant` clippy warning. Call sites are unchanged because `Box<Meta>` auto-derefs.

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

Public Python-facing names use snake_case matching the original `pjsekai.scores` API where possible. The Python packages on PyPI are `pjsekai-scores-rs` and `pjsekai-scores-rs-skia-image`; both import as `import pjsekai_scores_rs`. Key differences from the Python original that must be preserved:

- `Score.set_meta(**kwargs)` (not attribute assignment)
- `Rebase.from_dict(d).apply(score)` (not `load_from_dict` / `rebase`)
- `Drawing.svg(score)` returns `str` (not `svgwrite.Drawing`)
- `Lyric.load(string)` (not file object)
- `score.events()` is a method (not attribute)
- `Drawing(generator=…)` and `sus_to_svg(generator=…)` accept an optional generator name (default `"HarukiBot NEO"`)
- `Score.open(path)` auto-detects `.sus` and custom chart JSON. Use `Score.open_sus()` / `Score.from_str()` or `Score.open_json()` / `Score.from_json()` when the format must be explicit.

---

## Build & test

```bash
cargo build --release                   # Rust crate + CLI (bin: pjsekai-scores-rs)
cargo test                              # Rust unit tests
cargo clippy -- -D warnings             # Lint (must be clean)
maturin build --release -i python3.14t  # Python 3.14t wheel
pip install pjsekai-scores-rs           # Install from PyPI
uv pip install target/wheels/*.whl      # Install local wheel into venv
```

Benchmarking (measured 2026-04-25):  
**Parse: 14.5ms → 3.3ms (4.4×) · Render: 382.7ms → 20.2ms (19.0×) · Total: 404.8ms → 23.2ms (17.4×)**  
Environment: Mac mini M4 · macOS 26.4.1 · Python 3.13 · ARM64

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
