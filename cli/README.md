# pjsekai-scores-rs-cli

Command-line renderer for Project SEKAI charts (`.sus` and custom chart JSON) to SVG, PNG
and JPEG, built on the [`pjsekai-scores-rs`](https://crates.io/crates/pjsekai-scores-rs)
library. The binary is called `pjsekai-scores-rs`.

```bash
cargo install pjsekai-scores-rs-cli
pjsekai-scores-rs master.sus --font-path /path/to/font.otf -o master.png
```

PNG/JPEG output (the default `image` feature) is pure Rust and needs no system libraries.
Pass the fonts to draw with in `--font-path` / `--font-dir`, or install with
`--features system-fonts` to also resolve CSS font families from the system fonts.
`cargo install pjsekai-scores-rs-cli --no-default-features` builds an SVG-only CLI.

Prebuilt binaries are attached to the
[GitHub releases](https://github.com/Team-Haruki/pjsekai-scores-rs/releases). See the
[repository README](https://github.com/Team-Haruki/pjsekai-scores-rs#cli-usage) for all
options.
