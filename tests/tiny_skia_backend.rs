//! tiny-skia raster backend: output shape, encoders, and similarity to Skia.
//!
//! The Skia comparisons need both `skia-image` and `tiny-skia-image`. They use a
//! synthetic chart with one system TrueType font registered on both backends, and
//! optionally the Haruki-Drawing-API chart fixture when
//! `PJSEKAI_SCORES_FIXTURE_DATA` (Drawing's `data/` directory) and
//! `PJSEKAI_SCORES_FIXTURE_FONTS` (a directory with the Source Han Sans SC
//! Regular/Bold/Heavy `.otf` files) are set.
#![cfg(feature = "tiny-skia-image")]

use pjsekai_scores_rs::tiny_skia_direct::{
    SkiaImageFormat, SkiaRasterColorType, score_to_skia_image, score_to_skia_raster,
};
use pjsekai_scores_rs::{Drawing, Score};

const CHART: &str = r##"
#TITLE "Tiny Skia"
#ARTIST "Haruki"
#DIFFICULTY 3
#PLAYLEVEL 30
#BPM01: 120
#BPM02: 180
#00008: 01
#00208: 02
#TIL01: "0'0:1.0,1'240:1.5,3'0:0.75"
#HISPEED 01
#00012: 14
#00014: 24
#00132A: 14
#00154: 34
#00236A: 24
#00216: 54
#00256: 44
#00093B: 12
#00195B: 32
#00297B: 22
#00312: 1121314151617181
#00452: 112131415161
#00110: 11
#00211: 11
"##;

/// Every text class the renderer draws, pointed at one font family.
fn font_css(family: &str) -> String {
    format!(
        ".title, .subtitle, .bar-count-text, .event-text, .speed-text, .lyric-text, \
         .tick-text, .skill-text, .fever-text {{ font-family: \"{family}\"; }}\n"
    )
}

fn vector_drawing(font: Option<(&str, &str)>) -> Drawing {
    let style = font.map(|(_, family)| font_css(family));
    let mut drawing = Drawing::new(
        // An http host disables note sprites (vector fallback), like sekai-assets-updater.
        Some("https://assets.example.test/notes".to_string()),
        style,
        false,
        None,
        None,
        None,
    );
    if let Some((path, _)) = font {
        drawing.set_font_paths([path]);
    }
    drawing
}

/// A TrueType font present on common CI and developer machines.
fn system_test_font() -> Option<(&'static str, &'static str)> {
    [
        (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "DejaVu Sans",
        ),
        (
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "Liberation Sans",
        ),
        ("/System/Library/Fonts/Supplemental/Arial.ttf", "Arial"),
        ("C:\\Windows\\Fonts\\arial.ttf", "Arial"),
    ]
    .into_iter()
    .find(|(path, _)| std::path::Path::new(path).exists())
}

#[test]
fn renders_premultiplied_rgba_raster_png_and_jpeg() {
    let mut score = Score::parse(CHART);
    let mut drawing = vector_drawing(system_test_font());
    let raster = score_to_skia_raster(&mut drawing, &mut score, None).expect("raster");
    assert!(raster.width > 0 && raster.height > 0);
    assert_eq!(raster.color_type, SkiaRasterColorType::Rgba8888);
    assert_eq!(raster.row_bytes, raster.width as usize * 4);
    assert_eq!(
        raster.pixels.len(),
        raster.row_bytes * raster.height as usize
    );
    // Premultiplied: no channel exceeds its alpha.
    assert!(
        raster
            .pixels
            .chunks_exact(4)
            .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
    );
    // Not blank: the lane, lines and notes add colours beyond the background.
    let distinct = raster
        .pixels
        .chunks_exact(4)
        .map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]))
        .collect::<std::collections::HashSet<_>>();
    assert!(distinct.len() > 16, "only {} colours", distinct.len());

    let mut score = Score::parse(CHART);
    let png =
        score_to_skia_image(&mut drawing, &mut score, None, SkiaImageFormat::Png).expect("png");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let decoder = png::Decoder::new(std::io::Cursor::new(&png));
    let reader = decoder.read_info().expect("png header");
    assert_eq!(
        (reader.info().width, reader.info().height),
        (raster.width as u32, raster.height as u32)
    );

    let mut score = Score::parse(CHART);
    let jpeg = score_to_skia_image(
        &mut drawing,
        &mut score,
        None,
        SkiaImageFormat::Jpeg { quality: 85 },
    )
    .expect("jpeg");
    assert!(jpeg.starts_with(&[0xff, 0xd8]));
    assert!(jpeg.ends_with(&[0xff, 0xd9]));
}

#[cfg(feature = "skia-image")]
mod versus_skia {
    use super::*;
    use pjsekai_scores_rs::skia_direct;

    struct Similarity {
        psnr: f64,
        over_32: f64,
    }

    /// RGBA8888 copy of a raster from either backend.
    fn rgba(pixels: &[u8], bgra: bool) -> Vec<u8> {
        let mut out = pixels.to_vec();
        if bgra {
            for p in out.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
        }
        out
    }

    fn similarity(a: &[u8], b: &[u8]) -> Similarity {
        assert_eq!(a.len(), b.len());
        let (mut sq, mut over_32) = (0.0_f64, 0_usize);
        for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            let mut max = 0;
            for c in 0..3 {
                let d = i32::from(pa[c]) - i32::from(pb[c]);
                sq += f64::from(d * d);
                max = max.max(d.abs());
            }
            if max > 32 {
                over_32 += 1;
            }
        }
        let pixels = (a.len() / 4) as f64;
        let mse = sq / (pixels * 3.0);
        Similarity {
            psnr: if mse == 0.0 {
                f64::INFINITY
            } else {
                10.0 * (255.0_f64 * 255.0 / mse).log10()
            },
            over_32: over_32 as f64 / pixels,
        }
    }

    fn compare(
        make: impl Fn() -> (Drawing, Score),
        expected_size: Option<(i32, i32)>,
    ) -> Similarity {
        let (mut drawing, mut score) = make();
        let skia =
            skia_direct::score_to_skia_raster(&mut drawing, &mut score, None).expect("skia raster");
        let (mut drawing, mut score) = make();
        let tiny = score_to_skia_raster(&mut drawing, &mut score, None).expect("tiny raster");
        assert_eq!((tiny.width, tiny.height), (skia.width, skia.height));
        if let Some(size) = expected_size {
            assert_eq!((tiny.width, tiny.height), size);
        }
        let skia_bgra = skia.color_type == skia_direct::SkiaRasterColorType::Bgra8888;
        similarity(&rgba(&skia.pixels, skia_bgra), &tiny.pixels)
    }

    // Loose bounds: Linux (FreeType) Skia measures ~41 dB / 0.1% on the fixture;
    // macOS Skia lays text out with CoreText (unrounded advances, other gamma), so
    // these thresholds leave room for that while still catching broken drawing.
    const MIN_PSNR: f64 = 24.0;
    const MAX_OVER_32: f64 = 0.02;

    #[test]
    fn tiny_skia_matches_skia_on_a_vector_chart() {
        let Some(font) = system_test_font() else {
            eprintln!("skipped: no known system TrueType font");
            return;
        };
        let similarity = compare(|| (vector_drawing(Some(font)), Score::parse(CHART)), None);
        assert!(
            similarity.psnr >= MIN_PSNR,
            "PSNR {:.2} dB",
            similarity.psnr
        );
        assert!(
            similarity.over_32 <= MAX_OVER_32,
            "{:.3}% pixels differ by more than 32",
            similarity.over_32 * 100.0
        );
    }

    #[test]
    fn tiny_skia_matches_skia_on_the_drawing_fixture() {
        let (Ok(data), Ok(fonts)) = (
            std::env::var("PJSEKAI_SCORES_FIXTURE_DATA"),
            std::env::var("PJSEKAI_SCORES_FIXTURE_FONTS"),
        ) else {
            eprintln!("skipped: PJSEKAI_SCORES_FIXTURE_DATA / _FONTS not set");
            return;
        };
        let make = || {
            let mut score = Score::open(&format!(
                "{data}/asset/jp-assets/startapp/music/music_score/0001_01/expert.txt"
            ))
            .expect("fixture chart");
            score.meta.title = Some("Tell Your World".into());
            score.meta.artist = Some("kz".into());
            score.meta.difficulty = Some("expert".into());
            score.meta.playlevel = Some("22".into());
            score.meta.jacket = Some(format!(
                "{data}/asset/jp-assets/startapp/music/jacket/jacket_s_001/jacket_s_001.png"
            ));
            let mut style =
                std::fs::read_to_string(format!("{data}/static_images/chart_asset/css/black.css"))
                    .expect("fixture css");
            style.push_str(&font_css("Source Han Sans SC"));
            let mut drawing = Drawing::new(
                Some(format!("{data}/static_images/chart_asset/notes")),
                Some(style),
                false,
                None,
                Some(6.0),
                None,
            );
            drawing.set_font_paths(
                ["Regular", "Bold", "Heavy"].map(|w| format!("{fonts}/SourceHanSansSC-{w}.otf")),
            );
            (drawing, score)
        };
        let similarity = compare(make, Some((5248, 2688)));
        eprintln!(
            "fixture: PSNR {:.2} dB, {:.3}% > 32",
            similarity.psnr,
            similarity.over_32 * 100.0
        );
        assert!(
            similarity.psnr >= MIN_PSNR,
            "PSNR {:.2} dB",
            similarity.psnr
        );
        assert!(similarity.over_32 <= MAX_OVER_32);
    }
}
