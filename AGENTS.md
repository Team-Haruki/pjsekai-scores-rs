# AGENTS.md — pjsekai-scores-rs

Guidance for AI coding agents working on this codebase.

## What this project is

A Rust rewrite of [pjsekai/scores](https://gitlab.com/pjsekai/scores) — a `.sus` / Project SEKAI custom chart parser, SVG chart renderer, and direct PNG/JPEG renderer (pure Rust: tiny-skia + skrifa). It ships as a **Rust crate**, **CLI**, and **Python extension wheel** (PyO3/maturin).

The original Python implementation lives at `../scores/` and is the reference for correctness. Do not modify it.

---

## Build commands

```bash
# Rust only (crate + CLI). Default features are empty (parser + SVG); `image` adds PNG/JPEG/raster
cargo build --release                                   # parser + SVG renderer only
cargo build --release --features image                  # what the release CLI binaries ship
cargo build --release --features image,system-fonts     # + fontdb system font fallback
cargo install pjsekai-scores-rs --features image        # CLI with image output from crates.io
cargo check
cargo check --features python
cargo check --target wasm32-unknown-unknown --features wasm --lib
cargo test
cargo test --features image                   # golden crops need Linux + DejaVu Sans
cargo test --features image,system-fonts
cargo test <test_name>                        # single test
cargo clippy --all-targets -- -D warnings     # CI requires clean (default features)
cargo clippy --all-targets --features image -- -D warnings                      # CI requires clean
cargo clippy --all-targets --features python -- -D warnings                     # CI requires clean
cargo clippy --all-targets --features python,image,system-fonts -- -D warnings  # CI requires clean
cargo fmt --all --check                       # CI requires clean
cargo fmt                                     # auto-format

# Python wheel (current platform, active venv); pyproject.toml enables python + image
maturin build --release
# or for development install:
maturin develop --release

# WebAssembly (SVG only; no image)
wasm-pack build --release --target web --features wasm

# Python 3.14t free-threaded wheel (macOS ARM64)
maturin build --release -i python3.14t

# Cross-compile for Linux x64
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.14 \
  maturin build --release --target x86_64-unknown-linux-gnu --zig -i python3.14t

# Cross-compile for Windows x64
PYO3_CROSS=1 PYO3_CROSS_PYTHON_VERSION=3.14 \
  maturin build --release --target x86_64-pc-windows-gnu -i python3.14t

# Deprecated pjsekai-scores-rs-skia-image shim (sdist + py3-none-any wheel)
bash .github/scripts/build-shim.sh dist
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
├── score_json.rs   Project SEKAI custom chart JSON parser (serde_json → Score)
├── lyric.rs        Lyric/Word parser
├── rebase.rs       BPM/timing rebase transformation
├── drawing.rs      SVG renderer — direct String building, ~1900 lines
├── tiny_skia_direct.rs  Direct PNG/JPEG/raster renderer (feature `image`) + CSS/font handling
├── tiny_skia_direct/    canvas.rs (SkCanvas-shaped wrapper), aaa.rs + raster.rs (Skia analytic AA emulation), text.rs (skrifa fonts, glyph masks), codec.rs (decode, PNG/JPEG encode), system_fonts.rs (feature `system-fonts`)
├── python.rs       All PyO3 bindings (PyFraction, PyMeta, PyEvent, PyScore, PyLyric, PyRebase, PyDrawing; PyRasterImage under `image`)
├── wasm.rs         wasm-bindgen bindings (Score, Drawing, Rebase, Lyric; SVG only)
├── notes.rs        NoteData enum, arena index pattern (NoteIdx = usize)
└── notes/
    ├── tap.rs          TapType (8 variants)
    ├── directional.rs  DirectionalType (6 variants)
    ├── slide.rs        SlideType + Bézier path data
    └── event.rs        Event (BPM / bar-length / speed / text)
tests/
├── core_flows.rs   Parser, timing, rebase and SVG flows
├── image_render.rs Raster/PNG/JPEG output and the golden crops (feature `image`)
└── golden/         Linux Skia crops of the synthetic test chart
python/skia-image-shim/  Deprecated `pjsekai-scores-rs-skia-image` package: metadata only, depends on `pjsekai-scores-rs>=0.6.0`
```

---

## Key architectural decisions

### `Score::parse` and `impl std::str::FromStr`
`Score` implements `std::str::FromStr`. Use `Score::parse(content)` as the public Rust method, or `content.parse::<Score>()` via the trait. The Python binding `Score.from_str(s)` delegates to `s.parse::<Score>().unwrap()`.

### WebAssembly API
The `wasm` feature is independent from `python` and `image`; build it with just `--features wasm` (the default features are empty). It exposes `Score`, `Drawing`, `Rebase`, and `Lyric` through `wasm-bindgen` for in-memory `.sus` / custom-chart JSON parsing and SVG string output. Keep browser-facing APIs content-based (`Score.fromSus`, `Score.fromJson`, `Score.load`, `Drawing.svg`) instead of file-path based; `Score::open*`, CLI code, local font scanning, and raster output are not part of the wasm surface.

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

### SVG rendering (drawing.rs)
The SVG output is built directly into a `String` via `std::fmt::Write` — there is no DOM library. Only the default theme (`css/default.css`) is embedded at compile time with `include_str!`; the other files in `css/` (`black`, `white`, `guess`, `color`) are not referenced by the code and are used by passing them as a custom stylesheet (CLI `--css`, Python `style_sheet=`), which is appended after the default theme.

### Drawing.svg() borrow order
`self.build_skill_covers(score)` takes `&mut self`. The `let cfg = &self.config` binding must come **after** this mutable call, not before. Violating this causes E0502.

### Custom fonts and CSS `font-family`
Image output honors CSS `font-family`, `font-weight`, and `font-size` parsed from the built-in theme plus runtime `style_sheet`. The SVG renderer only emits CSS; custom font loading is for direct PNG/JPEG rendering.

Custom fonts enter through `DrawingConfig.font_paths` / `font_dirs`, CLI `--font-path` / `--font-dir`, and Python `Drawing(..., font_paths=..., font_dirs=...)` plus setter methods. `font_dirs` are scanned recursively for `.ttf`, `.otf`, and `.ttc`; prefer explicit `font_paths` in services to avoid scanning large asset roots on hot paths.

`tiny_skia_direct/text.rs` registers custom typefaces by their (localized) family names and PostScript name, all normalized for lookup. This lets CSS names such as `FOT-RodinNTLG Pro DB` and `Source Han Sans SC` match bundled fonts. When CJK text is present, a candidate typeface must cover the required glyphs before it is selected.

Custom font data is cached per process in `CUSTOM_FONT_CACHE`, keyed by sorted font path, modified time, and file size. Keep that key stable when changing font loading; stale font cache bugs are harder to diagnose than a small setup cost.

### Raster renderer (opt-in feature `image`)
`tiny_skia_direct.rs` renders PNG/JPEG/raster output on tiny-skia + skrifa. It replaced the Skia (skia-safe) renderer in 0.6.0 and keeps its layout code function by function; `canvas.rs`, `text.rs`, `aaa.rs` and `raster.rs` reproduce what Skia drew on Linux. The crate-level API keeps the 0.5 names (`score_to_skia_*`, `SkiaDirectError`, `SkiaRasterOutput`, ...), and `RASTER_BACKEND` is `"tiny-skia"`. `image` is opt-in so library and wasm users get no tiny-skia; the shipped artifacts turn it on (`pyproject.toml` features `["python", "image"]`, `features: --features image` on the `binaries` job in `release.yml`). Keep both of those, keep the default build parser/SVG-only, keep a CLI without `image` compiling (PNG/JPEG output then fails with "rebuild with `--features image`"), and keep `--features image` free of C build steps (no `cc`, `cmake`, `bindgen` or C `-sys` crates; check with `cargo tree -e normal,build --features image`).

Fidelity work is Linux-Skia-specific on purpose, and mostly emulates what Skia actually rasterizes rather than the ideal shape:
- **Paths** (`aaa.rs` + `raster.rs`): coverage is exact area (signed-area accumulation), computed over the edges Skia's analytic AA builds: y snapped to 1/4 px, slopes from x deltas truncated to 1/64 px, cubics and quads flattened by Skia's fixed-point forward differencing, `keepContinuous` drift for non-convex paths, `SkEdgeClipper` chops for paths that leave the clip, and the walker's non-zero intervals per quarter-pixel strip. The drift alone moves long eased slides by up to 0.5 px, so do not "fix" it.
- **Text** (`text.rs`): rounded (hinted) advances, glyph origins snapped to 1/4 px on the advance axis (quarter turns are snapped exactly, as `SkMatrix::setRotate` does, so rotated text keeps hinting), skrifa hinting, per-glyph A8 masks in a process-wide strike cache. Regular glyphs are rasterized like FreeType's `ftgrays` (26.6 points, its curve bisection, its coverage rounding); fake bold is Skia's stroke-and-fill path (miter join, `size * lerp(1/24, 1/32)` between 9 and 36 px) through the analytic AA emulation. Masks go through Skia's A8 gamma pre-blend (sRGB, contrast 0.5).
- **Images**: translate-only image draws use a custom bilinear blit because tiny-skia would switch them to nearest-neighbour.

Fonts: only `font_paths` / `font_dirs`, plus the system fonts through fontdb with the `system-fonts` feature (`system_fonts.rs`, scanned lazily once per process); with neither, rendering returns `SkiaDirectError::NoFonts` rather than silently dropping text. JPEG is mozjpeg-rs in its libjpeg-turbo-compatible mode (one interleaved baseline scan; jpeg-encoder's optimized output was three scans that zune-jpeg misreads). PNG is the `png` crate with the Up filter and zlib-rs deflate level 2: on chart pages it is as fast as the old multithreaded mtpng encoder at a fraction of the CPU, and smaller. Note sprites and recent jackets are decoded once per process (`NOTE_ASSET_CACHE`, `JACKET_CACHE`, keyed by path, modified time and size).

`tests/golden/*.png` are crops of the synthetic test chart as Linux Skia rendered it with DejaVu Sans 2.37 before 0.6.0: the only remaining Skia reference, so keep them. `PJSEKAI_SCORES_UPDATE_GOLDEN=1 cargo test --features image golden` on Linux overwrites them with the current output; do that only for an intended rendering change and say so in the PR. Keep `tests/image_render.rs` green; set `PJSEKAI_SCORES_FIXTURE_DATA` / `PJSEKAI_SCORES_FIXTURE_FONTS` to also run the Drawing API chart fixture.

### Raw strings with `href="#`
The literal `href="#` contains `"#` which prematurely closes `r#"..."#` raw strings. Use `r##"..."##` for any format string containing this pattern.

---

## Python bindings (python.rs)

### Feature gate
All PyO3 code is behind `#[cfg(feature = "python")]`. The crate builds as a pure Rust library + CLI without it.

### Thin binding layers
`python.rs` and `wasm.rs` are glue only — all business logic belongs in the core modules and both surfaces route through `Score`, `Drawing`, `Rebase`, and `Lyric`. When changing rendering arguments, keep `font_paths` / `font_dirs` exposed on `Drawing`, on `score_to_svg/png/jpg/jpeg`, and on the backward-compatible `sus_to_*` helpers.

### `Drawing.raster()`
The zero-copy service integration API: it renders into a native N32 premultiplied buffer owned by `RasterImage` and exposes a read-only Python buffer view. Use PNG/JPEG instead whenever the output has to cross a process or network boundary.

### Free-threaded Python (3.13t / 3.14t)
All `#[pyclass]` types own their data (no `Rc`/`RefCell`) so they are `Send + Sync` automatically. PyO3 0.29 supports free-threaded Python natively.

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
| system fonts only for raster text | `font_paths` / `font_dirs` for PNG/JPEG (wheels have no system-font fallback) |

---

## Things to avoid

- **Do not add `Rc` or `RefCell`** — breaks free-threaded Python compatibility.
- **Do not modify `../scores/`** — it is the reference Python implementation.
- **Do not call `maturin develop` with the 3.14t venv** — it fails; use `maturin build -i python3.14t` then `uv pip install`.
- **Do not use `r#"..."#`** for strings that embed `href="#` — use `r##"..."##`.
- **Do not move `let cfg = &self.config`** before the `build_skill_covers()` call in `drawing.rs`.
- **Do not rename `notes.rs` back to `notes/mod.rs`** — the module root lives at `src/notes.rs`; submodules stay in `src/notes/`.
- **Do not rely on host-installed fonts for deployed image output** — pass font files or directories; the PyPI wheels have no system-font fallback.
- **Do not point `font_dirs` at broad asset roots in performance-sensitive services** unless that scan cost is acceptable. Prefer known `font_paths`.
- **Do not make `RasterImage` writable** — `Drawing.raster()` is borrowed across free-threaded extension boundaries, so its buffer must remain immutable and alive for the full consumer view.

---

## Release and verification notes

- Release commits and release tags must be GPG-signed. Verify with `git log -1 --show-signature` and `git tag -v vX.Y.Z`.
- Pushing the tag runs one `Release` workflow (CLI binaries, the `pjsekai-scores-rs` wheels, the `pjsekai-scores-rs-skia-image` shim, crate). Check its PyPI job uploaded `pjsekai-scores-rs`; the shim stays at 0.6.0, so after 0.6.0 its upload is skipped as existing.
- For rendering/font/API changes, run `cargo test --features image`, `cargo test --features image,system-fonts` and `cargo check --features python` before release.
- For `RasterImage` changes, also build the Python wheel and verify `memoryview(raster).readonly` plus the downstream zero-copy consumer path (`.github/scripts/wheel_smoke.py` covers the first part).
- When changing CLI/Python options, update `README.md` and `AGENTS.md` in the same docs pass.

## Release notes

Release notes follow the org standard
[RELEASE_NOTES.md](https://github.com/seiunx-dev/ci-templates/blob/main/RELEASE_NOTES.md),
written in English.

- Title every release with the tag only, for example `v0.5.0`.
- Publish tags with an `-alpha`, `-beta` or `-rc` suffix as pre-releases; every other tag is a regular release.
- Omit empty sections, and end every item with its PR number `(#123)` (short commit SHA when there is no PR).
- The `Release` workflow publishes auto-generated notes; once it has published, rewrite them to the standard with `gh release edit <tag> --notes-file <file>`.

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
    default features (parser + SVG), `--features image`, `--features python` and
    `--features python,image,system-fonts`; `cargo check --target wasm32-unknown-unknown
    --features wasm`; a debug build of the CLI for `x86_64-unknown-linux-musl` with
    `image,system-fonts`, checked to be statically linked and to render PNG/JPEG; the tests
    run under `cargo llvm-cov` for the default features, `--features image` and
    `--features image,system-fonts`.
  - `Wheel smoke` (`maturin-wheels`): one linux-x64 `pjsekai-scores-rs` wheel, installed,
    imported and run through `.github/scripts/wheel_smoke.py` (SVG, PNG, JPEG, raster).
  - `Skia-image shim` (custom job, no template builds pure-Python packages): builds and
    checks the deprecated `pjsekai-scores-rs-skia-image` shim with
    `.github/scripts/build-shim.sh`.
  - `Sonar` scans the coverage (skipped green on Dependabot/fork PRs); `Workflow lint`
    runs actionlint.
- The aggregate job **`CI OK`** summarises the run and is what `release-gate` waits for.
  `main` currently has no branch protection or ruleset, so no status check is enforced
  on merge; check `CI OK` yourself before merging.
- `release.yml` (`Release`) replaces the old `release.yml` + `release-crate.yml` +
  `release-python.yml` (which rewrote the version from the tag with `sed`). Bump
  `version` in **both** `Cargo.toml` and `pyproject.toml` (and the package's own entry in
  `Cargo.lock`) in a PR → merge and wait for `CI OK` on `main` → push the signed tag
  `v<version>`. Pushing the tag creates the GitHub Release; do not create it by hand, but rewrite
  its notes afterwards (see [Release notes](#release-notes)).
  `release-gate` refuses a tag that differs from `Cargo.toml`/`pyproject.toml` and waits
  for `CI OK` on the tagged commit. Then, in one run:
  - the CLI binaries `pjsekai-scores-rs-{linux-x64,macos-arm64}.tar.gz` and
    `-windows-x64.zip` (`rust-release`, flat layout as before, built with `--features image`);
  - the `pjsekai-scores-rs` wheels (features `python` + `image` from `pyproject.toml`; not
    abi3: one wheel per interpreter, linux x64/arm64 in the manylinux container plus a
    manylinux_2_28 3.14t leg, macOS arm64/x64 and Windows x64 for 3.9–3.14t; the tested
    targets run `wheel_smoke.py`) and the sdist;
  - the `pjsekai-scores-rs-skia-image` 0.6.0 shim (`python/skia-image-shim`, sdist +
    py3-none-any wheel, custom `shim` job);
  - the GitHub Release with the binaries and `SHA256SUMS-<tag>.txt`;
  - PyPI (trusted publishing, environment `pypi`; both PyPI projects must name this
    repository, `release.yml` and `pypi` as trusted publisher) for the wheels and the
    shim, and crates.io
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
