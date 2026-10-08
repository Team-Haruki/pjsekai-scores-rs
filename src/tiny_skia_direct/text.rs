//! Font loading, glyph outlines and metrics via skrifa, shaped like the
//! `SkTypeface` / `SkFont` subset the renderer uses.
//!
//! Text layout matches what the Skia backend does on Linux (FreeType, normal
//! hinting, subpixel positioning, no shaping):
//!
//! - one glyph per `char` through the cmap, no fallback, missing glyphs as `.notdef`;
//! - advances are the hinted (rounded to whole pixels) advances;
//! - glyph origins are snapped like Skia's glyph cache does: a quarter pixel on the
//!   advance axis, a whole pixel on the other axis;
//! - outlines are hinted by skrifa's PostScript/TrueType hinter (the counterpart
//!   of FreeType's engines) unless `PJSEKAI_SCORES_TINY_SKIA_HINTING=0`;
//! - glyph coverage goes through Skia's A8 mask pre-blend (sRGB gamma, contrast
//!   0.5, keyed on the text colour's luminance in 3 bits), which makes light text
//!   on dark backgrounds heavier and dark text on light backgrounds lighter;
//! - fake bold grows the outline by Skia's `SkScalerContext` fake-bold amount,
//!   `size * lerp(1/24, 1/32)` between 9 and 36 px, offsetting points along the
//!   corner bisectors (FreeType's `FT_Outline_EmboldenXY`, kept centred).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, HintingInstance, HintingOptions, OutlinePen};
use skrifa::raw::{FileRef, TableProvider};
use skrifa::string::StringId;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{Path, PathBuilder, Transform};

pub(super) type Unichar = i32;

static HINTING_ENABLED: OnceLock<bool> = OnceLock::new();

fn hinting_enabled() -> bool {
    *HINTING_ENABLED.get_or_init(|| {
        !std::env::var("PJSEKAI_SCORES_TINY_SKIA_HINTING")
            .ok()
            .is_some_and(|value| matches!(value.as_str(), "0" | "false" | "False"))
    })
}

/// Skia's default `SK_GAMMA_CONTRAST`; `SK_GAMMA_EXPONENT` 0 selects sRGB.
const MASK_GAMMA_CONTRAST: f32 = 0.5;
/// `SkTMaskGamma<3, 3, 3>`: the text luminance is quantized to 3 bits.
const LUMINANCE_BITS: u32 = 3;

static MASK_GAMMA_LUTS: OnceLock<Vec<[u8; 256]>> = OnceLock::new();

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// `SkTMaskGamma_build_correcting_lut` for a source luminance byte.
fn build_correcting_lut(src_byte: u8) -> [u8; 256] {
    let src = f32::from(src_byte) / 255.0;
    let lin_src = srgb_to_linear(src);
    let dst = 1.0 - src;
    let lin_dst = srgb_to_linear(dst);
    let contrast = MASK_GAMMA_CONTRAST * lin_dst;
    let mut table = [0_u8; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let raw = i as f32 / 255.0;
        let srca = raw + (1.0 - raw) * contrast * raw;
        let value = if (src - dst).abs() < 1.0 / 256.0 {
            srca
        } else {
            let lin_out = lin_src * srca + (1.0 - srca) * lin_dst;
            (linear_to_srgb(lin_out) - dst) / (src - dst)
        };
        *entry = (255.0 * value).round().clamp(0.0, 255.0) as u8;
    }
    table
}

/// The coverage remap Skia applies to anti-aliased A8 glyph masks drawn in `color`.
pub(super) fn glyph_coverage_lut(color: tiny_skia::Color) -> &'static [u8; 256] {
    let luts = MASK_GAMMA_LUTS.get_or_init(|| {
        let levels = 1_u32 << LUMINANCE_BITS;
        (0..levels)
            .map(|i| {
                // sk_t_scale255: replicate the index bits across the byte.
                let byte = (i << (8 - LUMINANCE_BITS)) | (i << (8 - 2 * LUMINANCE_BITS)) | (i >> 1);
                build_correcting_lut(byte as u8)
            })
            .collect()
    });
    // SkColorSpaceLuminance (sRGB): Rec. 709 luma of the linearized colour.
    let luma = 0.2126 * srgb_to_linear(color.red())
        + 0.7152 * srgb_to_linear(color.green())
        + 0.0722 * srgb_to_linear(color.blue());
    let byte = (linear_to_srgb(luma) * 255.0).round().clamp(0.0, 255.0) as u32;
    &luts[(byte >> (8 - LUMINANCE_BITS)) as usize]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Slant {
    Upright,
    Italic,
    Oblique,
}

/// `SkFontStyle`: weight 1..1000, width class 1..9, slant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FontStyle {
    weight: i32,
    width: i32,
    slant: Slant,
}

impl FontStyle {
    pub(super) fn normal() -> Self {
        Self {
            weight: 400,
            width: 5,
            slant: Slant::Upright,
        }
    }

    pub(super) fn bold() -> Self {
        Self {
            weight: 700,
            ..Self::normal()
        }
    }

    pub(super) fn weight(&self) -> i32 {
        self.weight
    }

    pub(super) fn width(&self) -> i32 {
        self.width
    }

    pub(super) fn slant(&self) -> Slant {
        self.slant
    }
}

struct TypefaceData {
    data: Arc<Vec<u8>>,
    index: u32,
    style: FontStyle,
    family_names: Vec<String>,
    family_name: String,
    post_script_name: Option<String>,
}

/// A face inside a loaded font file (`SkTypeface`).
#[derive(Clone)]
pub(super) struct Typeface(Arc<TypefaceData>);

impl Typeface {
    fn new(data: Arc<Vec<u8>>, index: u32) -> Option<Self> {
        let font = FontRef::from_index(&data, index).ok()?;
        let os2 = font.os2().ok();
        let attributes = font.attributes();
        let style = FontStyle {
            weight: os2
                .as_ref()
                .map(|os2| i32::from(os2.us_weight_class()))
                .unwrap_or(attributes.weight.value().round() as i32),
            width: os2
                .as_ref()
                .map(|os2| i32::from(os2.us_width_class()).clamp(1, 9))
                .unwrap_or(5),
            slant: match attributes.style {
                skrifa::attribute::Style::Normal => Slant::Upright,
                skrifa::attribute::Style::Italic => Slant::Italic,
                skrifa::attribute::Style::Oblique(_) => Slant::Oblique,
            },
        };
        let mut family_names = Vec::new();
        for id in [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME] {
            for name in font.localized_strings(id) {
                let name = name.to_string();
                if !family_names.contains(&name) {
                    family_names.push(name);
                }
            }
        }
        let english = |id| {
            font.localized_strings(id)
                .english_or_first()
                .map(|name| name.to_string())
        };
        let family_name = english(StringId::TYPOGRAPHIC_FAMILY_NAME)
            .or_else(|| english(StringId::FAMILY_NAME))
            .unwrap_or_default();
        let post_script_name = english(StringId::POSTSCRIPT_NAME);
        Some(Self(Arc::new(TypefaceData {
            data,
            index,
            style,
            family_names,
            family_name,
            post_script_name,
        })))
    }

    fn font_ref(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.0.data, self.0.index).ok()
    }

    pub(super) fn font_style(&self) -> FontStyle {
        self.0.style
    }

    /// Localized typographic and legacy family names (`createFamilyNameIterator`).
    pub(super) fn family_names(&self) -> &[String] {
        &self.0.family_names
    }

    pub(super) fn family_name(&self) -> String {
        self.0.family_name.clone()
    }

    pub(super) fn post_script_name(&self) -> Option<String> {
        self.0.post_script_name.clone()
    }

    pub(super) fn unichar_to_glyph(&self, unichar: Unichar) -> u32 {
        let Some(font) = self.font_ref() else {
            return 0;
        };
        u32::try_from(unichar)
            .ok()
            .and_then(|ch| font.charmap().map(ch))
            .map_or(0, GlyphId::to_u32)
    }
}

/// Every face in a font file (`SkFontMgr::makeFromData` for each TTC index).
pub(super) fn load_typefaces_from_data(bytes: Vec<u8>) -> Vec<Typeface> {
    let data = Arc::new(bytes);
    let count = match FileRef::new(&data) {
        Ok(FileRef::Collection(collection)) => collection.len().min(32),
        Ok(FileRef::Font(_)) => 1,
        Err(_) => 0,
    };
    (0..count)
        .filter_map(|index| Typeface::new(Arc::clone(&data), index))
        .collect()
}

#[derive(Default)]
struct GlyphCache {
    hinting: Option<Option<HintingInstance>>,
    /// Outline in glyph space (y down, origin on the baseline), `None` if empty.
    outlines: HashMap<u32, Option<Path>>,
    advances: HashMap<u32, f32>,
}

/// A typeface at a size, with Skia's fake-bold flag (`SkFont`).
#[derive(Clone)]
pub(super) struct Font {
    typeface: Option<Typeface>,
    size: f32,
    embolden: bool,
    cache: Arc<Mutex<GlyphCache>>,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            typeface: None,
            size: 12.0,
            embolden: false,
            cache: Arc::default(),
        }
    }
}

impl Font {
    pub(super) fn new(typeface: Typeface, size: Option<f32>) -> Self {
        Self {
            typeface: Some(typeface),
            size: size.unwrap_or(12.0),
            ..Self::default()
        }
    }

    pub(super) fn set_size(&mut self, size: f32) {
        self.size = size;
        self.cache = Arc::default();
    }

    /// Positions are always subpixel; kept for parity with the Skia backend.
    pub(super) fn set_subpixel(&mut self, _subpixel: bool) {}

    pub(super) fn set_embolden(&mut self, embolden: bool) {
        self.embolden = embolden;
        self.cache = Arc::default();
    }

    fn glyph_ids(&self, font: &FontRef<'_>, text: &str) -> Vec<u32> {
        let charmap = font.charmap();
        text.chars()
            .map(|ch| charmap.map(ch).map_or(0, GlyphId::to_u32))
            .collect()
    }

    /// Advance width of `text` (`SkFont::measureText`).
    pub(super) fn measure_str(&self, text: &str) -> f32 {
        let Some(typeface) = &self.typeface else {
            return 0.0;
        };
        let Some(font) = typeface.font_ref() else {
            return 0.0;
        };
        let glyphs = self.glyph_ids(&font, text);
        let mut cache = self.cache.lock().expect("glyph cache lock poisoned");
        glyphs
            .iter()
            .map(|&gid| self.advance(&font, &mut cache, gid))
            .sum()
    }

    fn advance(&self, font: &FontRef<'_>, cache: &mut GlyphCache, gid: u32) -> f32 {
        *cache.advances.entry(gid).or_insert_with(|| {
            font.glyph_metrics(Size::new(self.size), LocationRef::default())
                .advance_width(GlyphId::new(gid))
                .unwrap_or(0.0)
                .round()
        })
    }

    /// Device-space outline of `text` drawn with its baseline origin at `origin`
    /// in local space, under `transform` (`SkCanvas::drawString`).
    pub(super) fn text_path(
        &self,
        text: &str,
        origin: (f32, f32),
        transform: Transform,
    ) -> Option<Path> {
        let typeface = self.typeface.as_ref()?;
        let font = typeface.font_ref()?;
        let glyphs = self.glyph_ids(&font, text);
        let mut cache = self.cache.lock().expect("glyph cache lock poisoned");
        let mut builder = PathBuilder::new();
        let mut pen_x = origin.0;
        for gid in glyphs {
            let advance = self.advance(&font, &mut cache, gid);
            if let Some(outline) = self.outline(&font, &mut cache, gid) {
                let mut glyph_origin = tiny_skia::Point::from_xy(pen_x, origin.1);
                transform.map_point(&mut glyph_origin);
                let (x, y) = snap_glyph_origin(&transform, glyph_origin.x, glyph_origin.y);
                let glyph_ts = Transform::from_row(
                    transform.sx,
                    transform.ky,
                    transform.kx,
                    transform.sy,
                    x,
                    y,
                );
                if let Some(path) = outline.clone().transform(glyph_ts) {
                    builder.push_path(&path);
                }
            }
            pen_x += advance;
        }
        builder.finish()
    }

    fn outline<'c>(
        &self,
        font: &FontRef<'_>,
        cache: &'c mut GlyphCache,
        gid: u32,
    ) -> Option<&'c Path> {
        if !cache.outlines.contains_key(&gid) {
            let outline = self.build_outline(font, cache, gid);
            cache.outlines.insert(gid, outline);
        }
        cache.outlines.get(&gid)?.as_ref()
    }

    fn build_outline(&self, font: &FontRef<'_>, cache: &mut GlyphCache, gid: u32) -> Option<Path> {
        let outlines = font.outline_glyphs();
        let glyph = outlines.get(GlyphId::new(gid))?;
        let size = Size::new(self.size);
        if cache.hinting.is_none() {
            cache.hinting = Some(if hinting_enabled() {
                HintingInstance::new(
                    &outlines,
                    size,
                    LocationRef::default(),
                    HintingOptions::default(),
                )
                .ok()
            } else {
                None
            });
        }
        let mut pen = ContourPen::default();
        let drawn = match cache.hinting.as_ref().and_then(Option::as_ref) {
            Some(instance) => glyph.draw(DrawSettings::hinted(instance, false), &mut pen),
            None => glyph.draw(
                DrawSettings::unhinted(size, LocationRef::default()),
                &mut pen,
            ),
        };
        if drawn.is_err() {
            pen = ContourPen::default();
            glyph
                .draw(
                    DrawSettings::unhinted(size, LocationRef::default()),
                    &mut pen,
                )
                .ok()?;
        }
        pen.finish_contour();
        if self.embolden {
            embolden(&mut pen.contours, fake_bold_extra(self.size) / 2.0);
        }
        pen.to_path()
    }
}

/// Skia's `SkScalerContext` fake-bold outset: the outline grows by
/// `size * lerp(1/24 at 9 px, 1/32 at 36 px)` in total.
fn fake_bold_extra(size: f32) -> f32 {
    const KEYS: [f32; 2] = [9.0, 36.0];
    const VALUES: [f32; 2] = [1.0 / 24.0, 1.0 / 32.0];
    let scale = if size <= KEYS[0] {
        VALUES[0]
    } else if size >= KEYS[1] {
        VALUES[1]
    } else {
        let t = (size - KEYS[0]) / (KEYS[1] - KEYS[0]);
        VALUES[0] + (VALUES[1] - VALUES[0]) * t
    };
    size * scale
}

/// Snaps a device-space glyph origin like Skia's glyph cache: subpixel (1/4 px)
/// along the axis the text advances on, whole pixels on the other axis, nothing
/// for skewed or arbitrarily rotated text.
fn snap_glyph_origin(ts: &Transform, x: f32, y: f32) -> (f32, f32) {
    let quarter = |v: f32| ((v + 0.125) * 4.0).floor() / 4.0;
    let whole = |v: f32| (v + 0.5).floor();
    if ts.kx == 0.0 && ts.ky == 0.0 {
        (quarter(x), whole(y))
    } else if ts.sx == 0.0 && ts.sy == 0.0 {
        (whole(x), quarter(y))
    } else {
        (x, y)
    }
}

#[derive(Clone, Copy)]
enum Verb {
    Line,
    Quad,
    Cubic,
}

#[derive(Default)]
struct Contour {
    /// Every on- and off-curve point in order, starting with the move-to point.
    points: Vec<(f32, f32)>,
    verbs: Vec<Verb>,
}

/// Collects glyph contours in font orientation (y up, pixels).
#[derive(Default)]
struct ContourPen {
    contours: Vec<Contour>,
    current: Option<Contour>,
}

impl ContourPen {
    fn finish_contour(&mut self) {
        if let Some(contour) = self.current.take()
            && !contour.verbs.is_empty()
        {
            self.contours.push(contour);
        }
    }

    fn push(&mut self, verb: Verb, points: &[(f32, f32)]) {
        let contour = self.current.get_or_insert_with(Contour::default);
        if contour.points.is_empty() {
            contour.points.push((0.0, 0.0));
        }
        contour.points.extend_from_slice(points);
        contour.verbs.push(verb);
    }

    /// Glyph-space path with y pointing down.
    fn to_path(&self) -> Option<Path> {
        let mut builder = PathBuilder::new();
        for contour in &self.contours {
            let p = |i: usize| (contour.points[i].0, -contour.points[i].1);
            let (x, y) = p(0);
            builder.move_to(x, y);
            let mut i = 1;
            for verb in &contour.verbs {
                match verb {
                    Verb::Line => {
                        let (x, y) = p(i);
                        builder.line_to(x, y);
                        i += 1;
                    }
                    Verb::Quad => {
                        let ((x1, y1), (x, y)) = (p(i), p(i + 1));
                        builder.quad_to(x1, y1, x, y);
                        i += 2;
                    }
                    Verb::Cubic => {
                        let ((x1, y1), (x2, y2), (x, y)) = (p(i), p(i + 1), p(i + 2));
                        builder.cubic_to(x1, y1, x2, y2, x, y);
                        i += 3;
                    }
                }
            }
            builder.close();
        }
        builder.finish()
    }
}

impl OutlinePen for ContourPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish_contour();
        self.current = Some(Contour {
            points: vec![(x, y)],
            verbs: Vec::new(),
        });
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push(Verb::Line, &[(x, y)]);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.push(Verb::Quad, &[(cx0, cy0), (x, y)]);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.push(Verb::Cubic, &[(cx0, cy0), (cx1, cy1), (x, y)]);
    }

    fn close(&mut self) {
        self.finish_contour();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Orientation {
    TrueType,
    PostScript,
}

/// `FT_Outline_Get_Orientation` over all contours (y up).
fn orientation(contours: &[Contour]) -> Option<Orientation> {
    let mut area = 0.0_f64;
    for contour in contours {
        let Some(&last) = contour.points.last() else {
            continue;
        };
        let mut prev = last;
        for &cur in &contour.points {
            area += f64::from(cur.1 - prev.1) * f64::from(cur.0 + prev.0);
            prev = cur;
        }
    }
    if area > 0.0 {
        Some(Orientation::PostScript)
    } else if area < 0.0 {
        Some(Orientation::TrueType)
    } else {
        None
    }
}

/// `FT_Outline_EmboldenXY` with `strength` already halved, minus FreeType's
/// up-right translation so the glyph grows symmetrically like Skia's stroke-and-fill.
fn embolden(contours: &mut [Contour], strength: f32) {
    if strength <= 0.0 {
        return;
    }
    let Some(orientation) = orientation(contours) else {
        return;
    };
    for contour in contours {
        let points = &mut contour.points;
        let n = points.len();
        if n < 2 {
            continue;
        }
        let last = n - 1;
        let next = |j: usize| if j < last { j + 1 } else { 0 };

        let mut l_in = 0.0_f32;
        let mut v_in = (0.0_f32, 0.0_f32);
        let mut anchor = (0.0_f32, 0.0_f32);
        let mut l_anchor = 0.0_f32;
        let mut k: Option<usize> = None;
        let mut i = last;
        let mut j = 0;
        // Counter j cycles through the points; i advances only when points move;
        // anchor k marks the first moved point.
        while j != i && Some(i) != k {
            let (v_out, l_out);
            if Some(j) != k {
                let dx = points[j].0 - points[i].0;
                let dy = points[j].1 - points[i].1;
                let len = (dx * dx + dy * dy).sqrt();
                if len == 0.0 {
                    j = next(j);
                    continue;
                }
                v_out = (dx / len, dy / len);
                l_out = len;
            } else {
                v_out = anchor;
                l_out = l_anchor;
            }

            if l_in != 0.0 {
                if k.is_none() {
                    k = Some(i);
                    anchor = v_in;
                    l_anchor = l_in;
                }
                let mut d = v_in.0 * v_out.0 + v_in.1 * v_out.1;
                let shift = if d > -0.9375 {
                    d += 1.0;
                    let mut shift = (v_in.1 + v_out.1, v_in.0 + v_out.0);
                    let mut q = v_out.0 * v_in.1 - v_out.1 * v_in.0;
                    if orientation == Orientation::TrueType {
                        shift.0 = -shift.0;
                        q = -q;
                    } else {
                        shift.1 = -shift.1;
                    }
                    let l = l_in.min(l_out);
                    let scale = if strength * q <= l * d {
                        strength / d
                    } else {
                        l / q
                    };
                    (shift.0 * scale, shift.1 * scale)
                } else {
                    (0.0, 0.0)
                };
                while i != j {
                    points[i].0 += shift.0;
                    points[i].1 += shift.1;
                    i = next(i);
                }
            } else {
                i = j;
            }
            v_in = v_out;
            l_in = l_out;
            j = next(j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Contour, Verb, embolden, fake_bold_extra, glyph_coverage_lut};

    #[test]
    fn fake_bold_matches_skia_interpolation() {
        assert!((fake_bold_extra(96.0) - 3.0).abs() < 1e-5);
        assert!((fake_bold_extra(6.0) - 0.25).abs() < 1e-5);
        let mid = fake_bold_extra(22.5);
        assert!((mid - 22.5 * (1.0 / 24.0 + 0.5 * (1.0 / 32.0 - 1.0 / 24.0))).abs() < 1e-5);
    }

    #[test]
    fn glyph_gamma_matches_skia_pre_blend() {
        let white = glyph_coverage_lut(tiny_skia::Color::WHITE);
        let black = glyph_coverage_lut(tiny_skia::Color::BLACK);
        // White text: coverage is sRGB-encoded; black text: contrast then inverse.
        assert_eq!(white[0], 0);
        assert_eq!(white[255], 255);
        assert_eq!(white[128], 188);
        assert_eq!(black[255], 255);
        assert!(black[128] < 100, "{}", black[128]);
    }

    #[test]
    fn embolden_grows_a_square_symmetrically() {
        // Counter-clockwise (PostScript) unit square scaled to 10 px, y up.
        let mut contours = vec![Contour {
            points: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            verbs: vec![Verb::Line, Verb::Line, Verb::Line],
        }];
        embolden(&mut contours, 1.0);
        let expected = [(-1.0, -1.0), (11.0, -1.0), (11.0, 11.0), (-1.0, 11.0)];
        for (point, expected) in contours[0].points.iter().zip(expected) {
            assert!(
                (point.0 - expected.0).abs() < 1e-4,
                "{point:?} vs {expected:?}"
            );
            assert!(
                (point.1 - expected.1).abs() < 1e-4,
                "{point:?} vs {expected:?}"
            );
        }
    }
}
