pub mod drawing;
pub mod fraction;
pub mod line;
pub mod lyric;
pub mod meta;
pub mod notes;
pub mod rebase;
pub mod score;
pub mod score_json;

#[cfg(feature = "image")]
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

// PNG/JPEG/raster rendering (feature `image`, on by default) on tiny-skia + skrifa.
#[cfg(feature = "image")]
pub use tiny_skia_direct::{
    JpegSubsampling, SkiaDirectError, SkiaImageFormat, SkiaImageOutput, SkiaRasterColorType,
    SkiaRasterOutput, SkiaRenderStats, score_to_skia_image, score_to_skia_image_with_stats,
    score_to_skia_jpeg, score_to_skia_jpeg_with_subsampling, score_to_skia_png,
    score_to_skia_raster,
};

/// The backend behind the raster API (`"tiny-skia"`).
#[cfg(feature = "image")]
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
