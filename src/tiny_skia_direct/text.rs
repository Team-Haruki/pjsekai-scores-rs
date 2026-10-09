//! Font loading, glyph outlines and metrics via skrifa, shaped like the
//! `SkTypeface` / `SkFont` subset the renderer uses.
//!
//! Text layout matches what Skia does on Linux (FreeType, normal
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
//! - glyphs are rasterized once per strike (typeface, size, fake bold, 2x2
//!   matrix) and subpixel position into A8 masks that are cached process-wide,
//!   like Skia's strike cache, and blitted one glyph at a time;
//! - regular glyphs are rasterized like FreeType's `ftgrays` (26.6 points, its
//!   curve flattening and coverage rounding);
//! - fake bold is Skia's `SkScalerContext` stroke-and-fill (`useStrokeForFakeBold`):
//!   the outline is stroked with a miter join at `size * lerp(1/24, 1/32)`
//!   between 9 and 36 px and the mask is drawn from that path by Skia's analytic
//!   AA (see `aaa`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, HintingInstance, HintingOptions, OutlinePen};
use skrifa::raw::{FileRef, TableProvider};
use skrifa::string::StringId;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{LineCap, LineJoin, Path, PathBuilder, PathSegment, Stroke, Transform};

use super::aaa;
use super::raster::{Quantize, Rasterizer};

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
    /// Process-unique id, the strike cache key.
    id: u64,
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
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Some(Self(Arc::new(TypefaceData {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
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

/// A rendered glyph: 8-bit coverage (before the gamma pre-blend) with its
/// top-left corner relative to the glyph's integral device origin.
pub(super) struct GlyphMask {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) coverage: Vec<u8>,
}

/// A glyph mask placed on the device: its origin is `(x, y)` in whole pixels.
pub(super) struct PlacedGlyph {
    pub(super) mask: Arc<GlyphMask>,
    pub(super) x: i64,
    pub(super) y: i64,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct StrikeKey {
    typeface: u64,
    size_bits: u32,
    embolden: bool,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct MaskKey {
    glyph: u32,
    /// The 2x2 part of the device matrix.
    matrix: [u32; 4],
    /// Subpixel position in quarter pixels.
    sub_x: u8,
    sub_y: u8,
}

/// Process-wide glyph cache for one typeface, size and fake-bold setting.
#[derive(Default)]
struct Strike {
    hinting: Option<Option<HintingInstance>>,
    /// Outlines in glyph space (y down, origin on the baseline), `None` if
    /// empty, keyed by glyph and whether they are hinted.
    outlines: HashMap<(u32, bool), Option<Path>>,
    advances: HashMap<u32, f32>,
    masks: HashMap<MaskKey, Option<Arc<GlyphMask>>>,
}

/// Strikes beyond this are dropped (all at once), like `CUSTOM_FONT_CACHE`.
const MAX_STRIKES: usize = 64;
/// Masks per strike beyond this are dropped (all at once).
const MAX_MASKS_PER_STRIKE: usize = 4096;

static STRIKES: LazyLock<Mutex<HashMap<StrikeKey, Arc<Mutex<Strike>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn strike(key: StrikeKey) -> Arc<Mutex<Strike>> {
    let mut strikes = STRIKES.lock().expect("strike cache lock poisoned");
    if !strikes.contains_key(&key) && strikes.len() >= MAX_STRIKES {
        strikes.clear();
    }
    Arc::clone(strikes.entry(key).or_default())
}

/// A typeface at a size, with Skia's fake-bold flag (`SkFont`).
#[derive(Clone)]
pub(super) struct Font {
    typeface: Option<Typeface>,
    size: f32,
    embolden: bool,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            typeface: None,
            size: 12.0,
            embolden: false,
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
    }

    /// Positions are always subpixel; kept for parity with `SkFont`.
    pub(super) fn set_subpixel(&mut self, _subpixel: bool) {}

    pub(super) fn set_embolden(&mut self, embolden: bool) {
        self.embolden = embolden;
    }

    fn strike(&self, typeface: &Typeface) -> Arc<Mutex<Strike>> {
        strike(StrikeKey {
            typeface: typeface.0.id,
            size_bits: self.size.to_bits(),
            embolden: self.embolden,
        })
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
        let strike = self.strike(typeface);
        let mut strike = strike.lock().expect("strike lock poisoned");
        glyphs
            .iter()
            .map(|&gid| self.advance(&font, &mut strike, gid))
            .sum()
    }

    fn advance(&self, font: &FontRef<'_>, strike: &mut Strike, gid: u32) -> f32 {
        *strike.advances.entry(gid).or_insert_with(|| {
            font.glyph_metrics(Size::new(self.size), LocationRef::default())
                .advance_width(GlyphId::new(gid))
                .unwrap_or(0.0)
                .round()
        })
    }

    /// The glyph masks of `text` drawn with its baseline origin at `origin` in
    /// local space, under `transform` (`SkCanvas::drawString`).
    pub(super) fn glyphs(
        &self,
        text: &str,
        origin: (f32, f32),
        transform: Transform,
    ) -> Vec<PlacedGlyph> {
        let Some(typeface) = self.typeface.as_ref() else {
            return Vec::new();
        };
        let Some(font) = typeface.font_ref() else {
            return Vec::new();
        };
        let glyphs = self.glyph_ids(&font, text);
        let strike = self.strike(typeface);
        let mut strike = strike.lock().expect("strike lock poisoned");
        let matrix = [transform.sx, transform.ky, transform.kx, transform.sy];
        let mut placed = Vec::with_capacity(glyphs.len());
        let mut pen_x = origin.0;
        for gid in glyphs {
            let advance = self.advance(&font, &mut strike, gid);
            let mut glyph_origin = tiny_skia::Point::from_xy(pen_x, origin.1);
            transform.map_point(&mut glyph_origin);
            pen_x += advance;
            let (x, y) = snap_glyph_origin(&transform, glyph_origin.x, glyph_origin.y);
            let (x_floor, y_floor) = (x.floor(), y.floor());
            let key = MaskKey {
                glyph: gid,
                matrix: matrix.map(f32::to_bits),
                sub_x: ((x - x_floor) * 4.0) as u8,
                sub_y: ((y - y_floor) * 4.0) as u8,
            };
            if strike.masks.len() >= MAX_MASKS_PER_STRIKE && !strike.masks.contains_key(&key) {
                strike.masks.clear();
            }
            let mask = match strike.masks.get(&key) {
                Some(mask) => mask.clone(),
                None => {
                    // Skia turns hinting off unless the text is axis-aligned
                    // (`SkTypeface_FreeType::onFilterRec`; 90-degree turns count).
                    let hinted = (transform.kx == 0.0 && transform.ky == 0.0)
                        || (transform.sx == 0.0 && transform.sy == 0.0);
                    let mask = self
                        .outline(&font, &mut strike, gid, hinted)
                        .cloned()
                        .and_then(|outline| {
                            self.render_mask(&outline, matrix, (x - x_floor, y - y_floor))
                        })
                        .map(Arc::new);
                    strike.masks.insert(key, mask.clone());
                    mask
                }
            };
            if let Some(mask) = mask {
                placed.push(PlacedGlyph {
                    mask,
                    x: x_floor as i64,
                    y: y_floor as i64,
                });
            }
        }
        placed
    }

    /// Rasterizes one glyph at a subpixel offset, the way Skia's FreeType
    /// scaler context does: FreeType's rasterizer for regular glyphs, Skia's own
    /// analytic AA for the stroked fake-bold path.
    fn render_mask(&self, outline: &Path, matrix: [f32; 4], sub: (f32, f32)) -> Option<GlyphMask> {
        let ts = Transform::from_row(matrix[0], matrix[1], matrix[2], matrix[3], sub.0, sub.1);
        let (device, freetype) = if self.embolden {
            (
                fake_bold(outline, fake_bold_extra(self.size))?.transform(ts)?,
                false,
            )
        } else {
            (outline.clone().transform(ts)?, true)
        };
        let bounds = device.bounds();
        let left = bounds.left().floor();
        let top = bounds.top().floor();
        let right = bounds.right().ceil();
        let bottom = bounds.bottom().ceil();
        let (width, height) = ((right - left) as usize, (bottom - top) as usize);
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return None;
        }
        let mut raster = Rasterizer::new(width, height);
        let quantize = if freetype {
            raster.add_path_freetype(&device, (left, top));
            Quantize::FreeType
        } else {
            // Skia draws the path into the mask with the mask's corner at 0, 0.
            let device = device.transform(Transform::from_translate(-left, -top))?;
            let clip = aaa::ClipRect {
                left: 0.0,
                top: 0.0,
                right: right - left,
                bottom: bottom - top,
            };
            aaa::add_path(&mut raster, &device, clip, (0, 0), true);
            Quantize::Round255
        };
        let mut coverage = vec![0_u8; width * height];
        for row in raster.rows() {
            raster.take_row(row, &mut coverage[row * width..][..width], quantize);
        }
        Some(GlyphMask {
            left: left as i32,
            top: top as i32,
            width,
            height,
            coverage,
        })
    }

    fn outline<'c>(
        &self,
        font: &FontRef<'_>,
        strike: &'c mut Strike,
        gid: u32,
        hinted: bool,
    ) -> Option<&'c Path> {
        if !strike.outlines.contains_key(&(gid, hinted)) {
            let outline = self.build_outline(font, strike, hinted);
            let outline = outline(gid);
            strike.outlines.insert((gid, hinted), outline);
        }
        strike.outlines.get(&(gid, hinted))?.as_ref()
    }

    fn build_outline<'f>(
        &self,
        font: &'f FontRef<'_>,
        strike: &'f mut Strike,
        hinted: bool,
    ) -> impl FnOnce(u32) -> Option<Path> + 'f {
        let size = Size::new(self.size);
        let outlines = font.outline_glyphs();
        if strike.hinting.is_none() {
            strike.hinting = Some(if hinting_enabled() {
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
        let hinting = strike
            .hinting
            .as_ref()
            .and_then(Option::as_ref)
            .filter(|_| hinted);
        move |gid| {
            let glyph = outlines.get(GlyphId::new(gid))?;
            let mut pen = ContourPen::default();
            let drawn = match hinting {
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
            pen.to_path()
        }
    }
}

/// Skia's stroke-and-fill fake bold (`SkStroke` with `fDoFill`): the outline
/// stroked with a miter join, plus the outline itself oriented like the
/// stroker's outer contours so the two add up under non-zero winding.
fn fake_bold(outline: &Path, extra: f32) -> Option<Path> {
    let stroke = Stroke {
        width: extra,
        miter_limit: 4.0,
        line_cap: LineCap::Butt,
        line_join: LineJoin::Miter,
        dash: None,
    };
    let stroked = outline.stroke(&stroke, 1.0)?;
    let mut builder = PathBuilder::new();
    // `SkPathPriv::ComputeFirstDirection(src) == kCCW` -> reverse the fill copy.
    if first_direction_is_ccw(outline) {
        push_reversed(&mut builder, outline);
    } else {
        builder.push_path(outline);
    }
    builder.push_path(&stroked);
    builder.finish()
}

/// Orientation of the contour holding the path's lowest point (largest y),
/// as Skia's `ComputeFirstDirection` decides it (y down: CCW on screen when
/// the cross product around that point is negative).
fn first_direction_is_ccw(path: &Path) -> bool {
    let mut best: Option<(f32, f64)> = None;
    for contour in contours(path) {
        let Some(max_y) = contour.iter().map(|p| p.y).reduce(f32::max) else {
            continue;
        };
        let mut area = 0.0_f64;
        for (i, p) in contour.iter().enumerate() {
            let q = contour[(i + 1) % contour.len()];
            area += f64::from(p.x) * f64::from(q.y) - f64::from(q.x) * f64::from(p.y);
        }
        if best.is_none_or(|(y, _)| max_y > y) {
            best = Some((max_y, area));
        }
    }
    best.is_some_and(|(_, area)| area < 0.0)
}

/// The on- and off-curve points of each contour.
fn contours(path: &Path) -> Vec<Vec<tiny_skia::Point>> {
    let mut out: Vec<Vec<tiny_skia::Point>> = Vec::new();
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => out.push(vec![p]),
            PathSegment::LineTo(p) => out.last_mut().into_iter().for_each(|c| c.push(p)),
            PathSegment::QuadTo(p1, p2) => {
                out.last_mut().into_iter().for_each(|c| c.extend([p1, p2]))
            }
            PathSegment::CubicTo(p1, p2, p3) => out
                .last_mut()
                .into_iter()
                .for_each(|c| c.extend([p1, p2, p3])),
            PathSegment::Close => {}
        }
    }
    out
}

/// Appends every contour of `path` with its direction reversed.
fn push_reversed(builder: &mut PathBuilder, path: &Path) {
    enum Seg {
        Line(tiny_skia::Point),
        Quad(tiny_skia::Point, tiny_skia::Point),
        Cubic(tiny_skia::Point, tiny_skia::Point, tiny_skia::Point),
    }
    let mut flush = |start: tiny_skia::Point, segs: &mut Vec<(tiny_skia::Point, Seg)>| {
        if segs.is_empty() {
            return;
        }
        let end = match segs.last() {
            Some((_, Seg::Line(p)))
            | Some((_, Seg::Quad(_, p)))
            | Some((_, Seg::Cubic(_, _, p))) => *p,
            None => start,
        };
        builder.move_to(end.x, end.y);
        for (from, seg) in segs.drain(..).rev() {
            match seg {
                Seg::Line(_) => builder.line_to(from.x, from.y),
                Seg::Quad(c, _) => builder.quad_to(c.x, c.y, from.x, from.y),
                Seg::Cubic(c1, c2, _) => builder.cubic_to(c2.x, c2.y, c1.x, c1.y, from.x, from.y),
            }
        }
        builder.close();
    };
    let mut start = tiny_skia::Point::zero();
    let mut last = start;
    let mut segs: Vec<(tiny_skia::Point, Seg)> = Vec::new();
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                flush(start, &mut segs);
                start = p;
                last = p;
            }
            PathSegment::LineTo(p) => {
                segs.push((last, Seg::Line(p)));
                last = p;
            }
            PathSegment::QuadTo(c, p) => {
                segs.push((last, Seg::Quad(c, p)));
                last = p;
            }
            PathSegment::CubicTo(c1, c2, p) => {
                segs.push((last, Seg::Cubic(c1, c2, p)));
                last = p;
            }
            PathSegment::Close => {
                if last != start {
                    segs.push((last, Seg::Line(start)));
                }
                flush(start, &mut segs);
                last = start;
            }
        }
    }
    flush(start, &mut segs);
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

#[cfg(test)]
mod tests {
    use super::{fake_bold, fake_bold_extra, glyph_coverage_lut};
    use tiny_skia::PathBuilder;

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
    fn fake_bold_grows_both_orientations_by_half_the_stroke() {
        for reversed in [false, true] {
            let mut b = PathBuilder::new();
            let corners = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
            let order: Vec<_> = if reversed {
                corners.iter().rev().collect()
            } else {
                corners.iter().collect()
            };
            b.move_to(order[0].0, order[0].1);
            for p in &order[1..] {
                b.line_to(p.0, p.1);
            }
            b.close();
            let bold = fake_bold(&b.finish().unwrap(), 2.0).unwrap();
            let bounds = bold.bounds();
            assert_eq!(
                (bounds.left(), bounds.top(), bounds.right(), bounds.bottom()),
                (-1.0, -1.0, 11.0, 11.0)
            );
            // The centre stays filled: the fill copy winds like the stroke.
            let mut r = super::Rasterizer::new(12, 12);
            r.add_path(&bold, tiny_skia::Transform::from_translate(1.0, 1.0));
            let mut row = vec![0; 12];
            for y in 0..12 {
                r.take_row(y, &mut row, super::Quantize::Round255);
                if y == 6 {
                    assert!(row.iter().all(|&c| c == 255), "{row:?}");
                }
            }
        }
    }
}
