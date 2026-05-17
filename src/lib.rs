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

#[cfg(feature = "skia-image")]
pub use skia_direct::{
    SkiaDirectError, SkiaImageFormat, SkiaImageOutput, SkiaRenderStats, score_to_skia_image,
    score_to_skia_image_with_stats, score_to_skia_jpeg, score_to_skia_png,
};

/// Python bindings via PyO3 (only compiled with `--features python`)
#[cfg(feature = "python")]
mod python;

#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "python")]
#[pymodule]
fn pjsekai_scores_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    python::register(m)
}
