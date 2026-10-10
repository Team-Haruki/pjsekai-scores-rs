# Copilot Instructions — pjsekai-scores-rs

## Project overview

Rust rewrite of the [pjsekai/scores](https://gitlab.com/pjsekai/scores) `.sus` parser, Project SEKAI custom chart JSON parser, and SVG chart renderer, plus a pure-Rust PNG/JPEG renderer (tiny-skia + skrifa, opt-in cargo feature `image`; the wheels and release CLI binaries include it). The CLI binary needs the opt-in `cli` feature (clap). Distributed as a Rust crate (`pjsekai-scores-rs`), Python wheels via PyO3 0.29 / maturin, and an SVG-only WebAssembly package via wasm-bindgen. The PyPI package is `pjsekai-scores-rs` (imports as `pjsekai_scores_rs`, image output always included); `pjsekai-scores-rs-skia-image` is a deprecated metadata-only shim (`python/skia-image-shim`) that depends on it.

**Do not modify `../scores/`** — it is the read-only reference Python implementation.

---

## Code style

- Use `rustfmt` defaults (no manual formatting rules).
- Prefer `impl From<X> for Y` over standalone conversion functions.
- Error types implement `Display` and `std::error::Error` by hand (there is no `thiserror` dependency); propagate with `?`.
- Keep `python.rs` as a thin binding layer — no business logic. All logic lives in the core modules.
- The default CSS theme (`css/default.css`) is embedded with `include_str!` at compile time.

---

## Architecture rules

### Notes use arena indexing
```rust
type NoteIdx = usize;
const NO_NOTE: NoteIdx = usize::MAX;
```
Cross-references between notes are stored as `NoteIdx` into `Score::notes: Vec<NoteData>`. Never introduce `Rc` or `RefCell` — they break PyO3 free-threaded compatibility. `Arc` is `Send + Sync` and is already used for the shared custom-font cache in `tiny_skia_direct.rs`.

### `#[cfg(feature = "python")]` guards all PyO3 code
The crate must build as a pure Rust library without the `python` feature:
```bash
cargo check                        # parser + SVG only (default features)
cargo check --features image       # + PNG/JPEG/raster
cargo check --features python      # with PyO3
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

Public Python-facing names use snake_case matching the original `pjsekai.scores` API where possible. The Python package on PyPI is `pjsekai-scores-rs`; it imports as `import pjsekai_scores_rs`. Key differences from the Python original that must be preserved:

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
cargo build --release --features cli,image  # CLI with PNG/JPEG (bin: pjsekai-scores-rs, needs `cli`)
cargo test                              # Rust unit tests
cargo clippy --all-targets -- -D warnings                               # Lint (must be clean)
cargo clippy --all-targets --features image -- -D warnings                      # CI lints this set too
cargo clippy --all-targets --features cli,python,image,system-fonts -- -D warnings  # and this one
maturin build --release -i python3.14t  # Python 3.14t wheel
pip install pjsekai-scores-rs           # Install from PyPI
uv pip install target/wheels/*.whl      # Install local wheel into venv
```

Benchmarking (measured 2026-04-25):  
**Parse: 14.5ms → 3.3ms (4.4×) · Render: 382.7ms → 20.2ms (19.0×) · Total: 404.8ms → 23.2ms (17.4×)**  
Environment: Mac mini M4 · macOS 26.4.1 · Python 3.13 · ARM64

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
  - Claude (any model): `Co-authored-by: Claude Fable 5 <noreply@anthropic.com>` (substitute the actual model, e.g. `Claude Opus 4.7`, `Claude Sonnet 4.6`, `Claude Haiku 4.5`)
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

CI reuses the shared templates in
[`seiunx-dev/ci-templates`](https://github.com/seiunx-dev/ci-templates) at `@v1`.
The files in `.github/workflows` are thin callers:

- `ci.yml` (`CI`) runs on `main` pushes, pull requests targeting `main`, and manual
  dispatch:
  - `Rust` (`rust-ci`): `cargo fmt --check`; clippy `--all-targets -D warnings` for the
    default features (parser + SVG library), `--features image`, `--features cli`,
    `--features python` and `--features cli,python,image,system-fonts`; the wasm check
    (`--features wasm`); a static musl CLI build with `cli,image,system-fonts` that renders
    PNG/JPEG; the tests under `cargo llvm-cov` for the default features, `--features cli`,
    `--features cli,image` and `--features cli,image,system-fonts`.
  - `Wheel smoke` (`maturin-wheels`): one linux-x64 `pjsekai-scores-rs` wheel, installed,
    imported and run through `.github/scripts/wheel_smoke.py` (SVG, PNG, JPEG, raster).
  - `Skia-image shim`: builds and checks the deprecated `pjsekai-scores-rs-skia-image`
    shim with `.github/scripts/build-shim.sh`.
  - `Sonar` scans the coverage (skipped green on Dependabot/fork PRs); `Workflow lint`
    runs actionlint.
- The aggregate job **`CI OK`** summarises the run and is what `release-gate` waits for.
  `main` currently has no branch protection or ruleset, so no status check is enforced
  on merge; check `CI OK` yourself before merging.
- `release.yml` (`Release`) replaces the old `release.yml` + `release-crate.yml` +
  `release-python.yml` (which rewrote the version from the tag with `sed`). Bump
  `version` in **both** `Cargo.toml` and `pyproject.toml` (and the package's own entry in
  `Cargo.lock`) in a PR → merge and wait for `CI OK` on `main` → push the signed tag
  `v<version>`. Pushing the tag creates the GitHub Release; do not create it by hand.
  `release-gate` refuses a tag that differs from `Cargo.toml`/`pyproject.toml` and waits
  for `CI OK` on the tagged commit. Then, in one run:
  - the CLI binaries `pjsekai-scores-rs-{linux-x64,macos-arm64}.tar.gz` and
    `-windows-x64.zip` (`rust-release`, flat layout as before);
  - the `pjsekai-scores-rs` wheels (features `python` + `image`; not abi3: one wheel per
    interpreter, linux x64/arm64 in the manylinux container plus a manylinux_2_28 3.14t
    leg, macOS arm64/x64 and Windows x64 for 3.9–3.14t) and the sdist;
  - the `pjsekai-scores-rs-skia-image` 0.6.0 shim (sdist + py3-none-any wheel);
  - the GitHub Release with the binaries and `SHA256SUMS-<tag>.txt`;
  - PyPI (trusted publishing, environment `pypi`) for both projects, and crates.io
    (environment `crates-io`, `CARGO_REGISTRY_TOKEN`).
  Manual dispatch is a dry run: it builds everything and publishes nothing.
- CI never rewrites `Cargo.toml` / `pyproject.toml` or regenerates `Cargo.lock`.

Workflow maintenance rules:

- Use the shared templates first. Add custom jobs or steps only when a template
  genuinely cannot meet the project's needs, keep them in the thin caller files, and
  add a comment explaining why. The PyPI and crates.io publish jobs live in
  `release.yml` because trusted publishing is bound to the calling workflow file.
- Template bugs and missing features are fixed upstream in `seiunx-dev/ci-templates`
  (new `v1.x.y` tag), not worked around here.
- Keep top-level `permissions: contents: read`; grant `contents: write` / `id-token: write`
  only on the job that needs it.
- Do not set `sonar.projectVersion` or `*.reportPaths` in `sonar-project.properties`, and
  do not suppress `githubactions:S7637` there: the template's `sonar.yml` passes the
  version (`project-version: auto`) and report paths, and ignores S7637 for the `@v1`
  references.
- Third-party actions in caller-side custom steps are pinned to a full commit SHA with a
  `# vX.Y.Z` comment; Dependabot (`github-actions`) updates them and the template refs.
