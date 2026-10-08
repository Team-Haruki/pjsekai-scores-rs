pub mod drawing;
pub mod fraction;
pub mod line;
pub mod lyric;
pub mod meta;
pub mod notes;
pub mod rebase;
pub mod score;
pub mod score_json;

#[cfg(feature = "skia-image")]
pub mod skia_direct;
#[cfg(feature = "tiny-skia-image")]
pub mod tiny_skia_direct;

// Re-exports for convenience
pub use drawing::{Drawing, MusicMeta};
pub use fraction::Fraction;
pub use lyric::Lyric;
pub use meta::Meta;
pub use notes::directional::{Directional, DirectionalType};
pub use notes::event::Event;
pub use notes::slide::{Slide, SlideType};
pub use notes::tap::{Tap, TapType};
pub use notes::{NoteData, NoteIdx};
pub use rebase::Rebase;
pub use score::Score;
pub use score_json::ScoreJsonError;

// The crate-level raster API comes from Skia when `skia-image` is on, otherwise from
// the pure-Rust tiny-skia backend. With both features, `tiny_skia_direct` keeps the
// same names under its own module path.
#[cfg(feature = "skia-image")]
pub use skia_direct::{
    JpegSubsampling, PngEncoder, SkiaDirectError, SkiaImageFormat, SkiaImageOutput,
    SkiaRasterColorType, SkiaRasterOutput, SkiaRenderStats, score_to_skia_image,
    score_to_skia_image_with_stats, score_to_skia_jpeg, score_to_skia_jpeg_with_subsampling,
    score_to_skia_png, score_to_skia_png_with_encoder, score_to_skia_raster,
};
#[cfg(all(feature = "tiny-skia-image", not(feature = "skia-image")))]
pub use tiny_skia_direct::{
    JpegSubsampling, PngEncoder, SkiaDirectError, SkiaImageFormat, SkiaImageOutput,
    SkiaRasterColorType, SkiaRasterOutput, SkiaRenderStats, score_to_skia_image,
    score_to_skia_image_with_stats, score_to_skia_jpeg, score_to_skia_jpeg_with_subsampling,
    score_to_skia_png, score_to_skia_png_with_encoder, score_to_skia_raster,
};

/// The backend behind the crate-level raster API (`"skia"` or `"tiny-skia"`).
#[cfg(feature = "skia-image")]
pub const RASTER_BACKEND: &str = "skia";
/// The backend behind the crate-level raster API (`"skia"` or `"tiny-skia"`).
#[cfg(all(feature = "tiny-skia-image", not(feature = "skia-image")))]
pub const RASTER_BACKEND: &str = tiny_skia_direct::BACKEND_NAME;

/// Python bindings via PyO3 (only compiled with `--features python`)
#[cfg(feature = "python")]
mod python;

#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "python")]
#[pymodule]
fn pjsekai_scores_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    python::register(m)
}
