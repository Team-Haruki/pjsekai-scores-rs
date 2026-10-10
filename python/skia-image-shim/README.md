# pjsekai-scores-rs-skia-image (deprecated)

This package is deprecated. Install [`pjsekai-scores-rs`](https://pypi.org/project/pjsekai-scores-rs/) instead:

```bash
pip uninstall pjsekai-scores-rs-skia-image
pip install "pjsekai-scores-rs>=0.6.0"
```

Since 0.6.0, the `pjsekai-scores-rs` wheels always include PNG/JPEG and raster
rendering. The Skia renderer was replaced by a pure-Rust one (tiny-skia + skrifa), so
there is no separate image package any more, and no FreeType, fontconfig or other
system library is needed.

The import name and the Python API are unchanged (`import pjsekai_scores_rs`). One
difference: the wheels have no system-font fallback. Pass the fonts to draw with in
`font_paths` / `font_dirs`; rendering text without any font raises an error.

## Upgrading an existing environment with pip

The 0.5 `pjsekai-scores-rs-skia-image` wheel installed the `pjsekai_scores_rs` module
itself. When pip upgrades it in place (`pip install -U pjsekai-scores-rs-skia-image`),
it installs `pjsekai-scores-rs` first and then removes the old wheel's files, which
deletes the module that `pjsekai-scores-rs` just installed. Uninstall the old package
first, as shown above, or repair the environment afterwards:

```bash
pip install --force-reinstall --no-deps "pjsekai-scores-rs>=0.6.0"
```

uv and fresh environments (such as Docker builds) are not affected.

## What this release contains

This 0.6.0 release contains no code. It only depends on `pjsekai-scores-rs>=0.6.0`, so
existing requirements on `pjsekai-scores-rs-skia-image` keep installing the
replacement. It receives no further updates: depend on `pjsekai-scores-rs` directly.
