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
    let Some(font) = system_test_font() else {
        eprintln!("skipped: no known system TrueType font");
        return;
    };
    let mut score = Score::parse(CHART);
    let mut drawing = vector_drawing(Some(font));
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
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
    );
    // Not blank: the lane, lines and notes add colours beyond the background.
    let distinct = raster
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
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
            for p in out.as_chunks_mut::<4>().0.iter_mut() {
                p.swap(0, 2);
            }
        }
        out
    }

    fn similarity(a: &[u8], b: &[u8]) -> Similarity {
        assert_eq!(a.len(), b.len());
        let (mut sq, mut over_32) = (0.0_f64, 0_usize);
        for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
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

    // On Linux both backends rasterize like FreeType + Skia's analytic AA: the
    // synthetic chart measures ~63 dB and the Drawing fixture ~54 dB, so a drop
    // below 50 dB is a regression. macOS Skia lays text out with CoreText
    // (unrounded advances, other gamma), so other platforms only catch broken
    // drawing.
    #[cfg(target_os = "linux")]
    const MIN_PSNR: f64 = 50.0;
    #[cfg(not(target_os = "linux"))]
    const MIN_PSNR: f64 = 24.0;
    #[cfg(target_os = "linux")]
    const MAX_OVER_32: f64 = 0.0005;
    #[cfg(not(target_os = "linux"))]
    const MAX_OVER_32: f64 = 0.02;

    #[test]
    fn tiny_skia_matches_skia_on_a_vector_chart() {
        let Some(font) = system_test_font() else {
            eprintln!("skipped: no known system TrueType font");
            return;
        };
        let similarity = compare(|| (vector_drawing(Some(font)), Score::parse(CHART)), None);
        eprintln!(
            "vector chart ({}): PSNR {:.2} dB, {:.3}% > 32",
            font.1,
            similarity.psnr,
            similarity.over_32 * 100.0
        );
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

    /// Without registered fonts both backends take text from the system fonts
    /// (fontconfig for Skia, fontdb for tiny-skia).
    #[cfg(feature = "system-fonts")]
    #[test]
    fn tiny_skia_matches_skia_with_system_fonts_only() {
        let similarity = compare(|| (vector_drawing(None), Score::parse(CHART)), None);
        eprintln!(
            "system fonts: PSNR {:.2} dB, {:.3}% > 32",
            similarity.psnr,
            similarity.over_32 * 100.0
        );
        // fontconfig and fontdb may still pick different faces for a family
        // list; this only proves that text is drawn and lands in the same place.
        assert!(similarity.psnr >= 24.0, "PSNR {:.2} dB", similarity.psnr);
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

/// Crops of the synthetic chart as Linux Skia renders it (FreeType, DejaVu Sans
/// 2.37), checked in under `tests/golden/`: they keep the tiny-skia backend's
/// fidelity testable without the Skia backend. Regenerate them with
/// `PJSEKAI_SCORES_UPDATE_GOLDEN=1 cargo test --features skia-image,tiny-skia-image
/// golden` on Linux.
const GOLDEN_CROPS: &[(&str, u32, u32, u32, u32)] = &[
    // Beat labels, speed text, notes with glow, grid lines.
    ("labels-and-notes", 50, 30, 230, 530),
    // Rotated bar-count/BPM text, a slide, tap and long notes.
    ("slide-and-bpm", 30, 2300, 256, 200),
    // Flick arrows and critical notes.
    ("flicks", 320, 2040, 96, 448),
];

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.png"))
}

/// Opaque RGB of a crop of an RGBA8888 (premultiplied, opaque) raster.
fn crop_rgb(pixels: &[u8], width: u32, bgra: bool, rect: (u32, u32, u32, u32)) -> Vec<u8> {
    let (x0, y0, w, h) = rect;
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let p = &pixels[((y * width + x) * 4) as usize..][..4];
            if bgra {
                out.extend_from_slice(&[p[2], p[1], p[0]]);
            } else {
                out.extend_from_slice(&p[..3]);
            }
        }
    }
    out
}

#[test]
fn matches_golden_skia_crops() {
    let Some(font) = system_test_font().filter(|font| font.1 == "DejaVu Sans") else {
        eprintln!("skipped: the golden crops were made with DejaVu Sans");
        return;
    };
    if !cfg!(target_os = "linux") {
        eprintln!("skipped: the golden crops follow Linux (FreeType) text rendering");
        return;
    }
    #[cfg(feature = "skia-image")]
    if std::env::var_os("PJSEKAI_SCORES_UPDATE_GOLDEN").is_some() {
        let mut drawing = vector_drawing(Some(font));
        let mut score = Score::parse(CHART);
        let skia =
            pjsekai_scores_rs::skia_direct::score_to_skia_raster(&mut drawing, &mut score, None)
                .expect("skia raster");
        let bgra = skia.color_type == pjsekai_scores_rs::skia_direct::SkiaRasterColorType::Bgra8888;
        eprintln!("chart is {}x{}", skia.width, skia.height);
        for &(name, x, y, w, h) in GOLDEN_CROPS {
            let rgb = crop_rgb(&skia.pixels, skia.width as u32, bgra, (x, y, w, h));
            let file = std::fs::File::create(golden_path(name)).expect("golden file");
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_compression(png::Compression::High);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&rgb).expect("png data");
        }
        return;
    }

    let mut drawing = vector_drawing(Some(font));
    let mut score = Score::parse(CHART);
    let tiny = score_to_skia_raster(&mut drawing, &mut score, None).expect("raster");
    for &(name, x, y, w, h) in GOLDEN_CROPS {
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(golden_path(name)).expect("golden crop"),
        ));
        let mut reader = decoder.read_info().expect("golden header");
        let mut expected = vec![0; reader.output_buffer_size().expect("size")];
        reader.next_frame(&mut expected).expect("golden data");
        let actual = crop_rgb(&tiny.pixels, tiny.width as u32, false, (x, y, w, h));
        assert_eq!(expected.len(), actual.len(), "{name}");
        let (mut sq, mut over_32) = (0.0_f64, 0_usize);
        for (e, a) in expected
            .as_chunks::<3>()
            .0
            .iter()
            .zip(actual.as_chunks::<3>().0)
        {
            let mut max = 0;
            for c in 0..3 {
                let d = i32::from(e[c]) - i32::from(a[c]);
                sq += f64::from(d * d);
                max = max.max(d.abs());
            }
            over_32 += usize::from(max > 32);
        }
        let mse = sq / expected.len() as f64;
        let psnr = if mse == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0_f64 * 255.0 / mse).log10()
        };
        eprintln!("golden {name}: PSNR {psnr:.2} dB, {over_32} px > 32");
        assert!(psnr >= 45.0, "{name}: PSNR {psnr:.2} dB");
        assert!(
            over_32 * 1000 <= (w * h) as usize,
            "{name}: {over_32} px > 32"
        );
    }
}
