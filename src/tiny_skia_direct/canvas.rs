//! A small Skia-`SkCanvas`-shaped wrapper over a tiny-skia pixmap.
//!
//! It provides exactly the canvas surface `tiny_skia_direct` needs (save/restore,
//! transforms, rect clips, rects, lines, round rects, paths, image rects and
//! glyph masks) and keeps Skia's semantics where tiny-skia differs:
//!
//! - axis-aligned rects and thick axis-aligned lines get exact rect coverage;
//! - other paths are filled with exact-area coverage over the edges Skia's
//!   analytic AA builds (`aaa`), not with tiny-skia's 4x4 supersampler;
//! - text is drawn as cached per-glyph A8 masks (`text`);
//! - translate-only image draws are bilinearly resampled at fractional offsets
//!   (tiny-skia would silently switch such patterns to nearest-neighbour).

use std::sync::Arc;

use super::aaa;
use super::raster::{Quantize, Rasterizer};
use super::text::PlacedGlyph;

use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Path, PathBuilder, Pattern,
    Pixmap, PixmapMut, Point, Shader, SpreadMode, Stroke, Transform,
};

/// A decoded, premultiplied RGBA image.
pub(super) type Image = Arc<Pixmap>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Rect {
    pub(super) fn from_xywh(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self::from_ltrb(x, y, x + width, y + height)
    }

    fn from_ltrb(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left: left.min(right),
            top: top.min(bottom),
            right: left.max(right),
            bottom: top.max(bottom),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        !(self.left < self.right && self.top < self.bottom)
    }

    pub(super) fn left(&self) -> f32 {
        self.left
    }

    pub(super) fn top(&self) -> f32 {
        self.top
    }

    pub(super) fn bottom(&self) -> f32 {
        self.bottom
    }

    fn width(&self) -> f32 {
        self.right - self.left
    }

    fn height(&self) -> f32 {
        self.bottom - self.top
    }

    fn intersect(&self, other: &Rect) -> Rect {
        Rect {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }

    /// Maps the rect through a scale/translate transform.
    fn map(&self, ts: &Transform) -> Rect {
        Rect::from_ltrb(
            self.left * ts.sx + ts.tx,
            self.top * ts.sy + ts.ty,
            self.right * ts.sx + ts.tx,
            self.bottom * ts.sy + ts.ty,
        )
    }

    fn to_tiny(self) -> Option<tiny_skia::Rect> {
        tiny_skia::Rect::from_ltrb(self.left, self.top, self.right, self.bottom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaintStyle {
    Fill,
    Stroke,
}

#[derive(Debug, Clone, Copy)]
struct LinearShader {
    start: (f32, f32),
    end: (f32, f32),
    colors: [Color; 2],
}

/// The subset of `SkPaint` the renderer uses.
#[derive(Debug, Clone)]
pub(super) struct Paint {
    color: Color,
    style: PaintStyle,
    stroke_width: f32,
    anti_alias: bool,
    shader: Option<LinearShader>,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            color: Color::BLACK,
            style: PaintStyle::Fill,
            stroke_width: 0.0,
            anti_alias: false,
            shader: None,
        }
    }
}

impl Paint {
    pub(super) fn set_anti_alias(&mut self, anti_alias: bool) {
        self.anti_alias = anti_alias;
    }

    pub(super) fn set_style(&mut self, style: PaintStyle) {
        self.style = style;
    }

    pub(super) fn set_color(&mut self, color: Color) {
        self.color = color;
    }

    pub(super) fn set_stroke_width(&mut self, width: f32) {
        self.stroke_width = width;
    }

    /// A two-stop linear gradient in local coordinates, clamped at both ends and
    /// interpolated in unpremultiplied space like Skia's default.
    pub(super) fn set_linear_gradient(
        &mut self,
        start: (f32, f32),
        end: (f32, f32),
        colors: [Color; 2],
    ) {
        self.shader = Some(LinearShader { start, end, colors });
    }

    fn to_tiny(&self) -> Option<tiny_skia::Paint<'static>> {
        let shader = match self.shader {
            Some(gradient) => LinearGradient::new(
                Point::from_xy(gradient.start.0, gradient.start.1),
                Point::from_xy(gradient.end.0, gradient.end.1),
                vec![
                    GradientStop::new(0.0, gradient.colors[0]),
                    GradientStop::new(1.0, gradient.colors[1]),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            )?,
            None => Shader::SolidColor(self.color),
        };
        Some(tiny_skia::Paint {
            shader,
            anti_alias: self.anti_alias,
            ..tiny_skia::Paint::default()
        })
    }

    fn stroke(&self) -> Stroke {
        Stroke {
            width: self.stroke_width,
            ..Stroke::default()
        }
    }
}

#[derive(Clone, Default)]
struct Clip {
    /// Device-space intersection of every axis-aligned clip rect.
    rect: Option<Rect>,
    /// Coverage of clips that were not axis-aligned in device space.
    path_mask: Option<Arc<Mask>>,
    /// Lazily built full coverage mask (rect ∩ path mask) for path draws.
    mask: Option<Arc<Mask>>,
}

#[derive(Clone)]
struct State {
    transform: Transform,
    clip: Clip,
}

/// A raster canvas drawing into a borrowed premultiplied RGBA8 buffer.
pub(super) struct Canvas<'a> {
    pixmap: PixmapMut<'a>,
    state: State,
    stack: Vec<State>,
}

impl<'a> Canvas<'a> {
    pub(super) fn new(pixmap: PixmapMut<'a>) -> Self {
        Self {
            pixmap,
            state: State {
                transform: Transform::identity(),
                clip: Clip::default(),
            },
            stack: Vec::new(),
        }
    }

    pub(super) fn transform(&self) -> Transform {
        self.state.transform
    }

    pub(super) fn clear(&mut self, color: Color) {
        self.pixmap.fill(color);
    }

    pub(super) fn save(&mut self) {
        self.stack.push(self.state.clone());
    }

    pub(super) fn restore(&mut self) {
        if let Some(state) = self.stack.pop() {
            self.state = state;
        }
    }

    pub(super) fn translate(&mut self, (dx, dy): (f32, f32)) {
        self.state.transform = self.state.transform.pre_translate(dx, dy);
    }

    pub(super) fn scale(&mut self, (sx, sy): (f32, f32)) {
        self.state.transform = self.state.transform.pre_scale(sx, sy);
    }

    /// Rotates by `degrees` (clockwise in device space, like `SkCanvas::rotate`).
    /// Like `SkMatrix::setRotate`, sines and cosines within 1/4096 of zero are
    /// snapped to zero, so quarter turns stay exactly axis-aligned (text then
    /// keeps hinting and subpixel positioning).
    pub(super) fn rotate(&mut self, degrees: f32) {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let snap = |v: f32| if v.abs() <= 1.0 / 4096.0 { 0.0 } else { v };
        let (sin, cos) = (snap(sin), snap(cos));
        self.state.transform = self
            .state
            .transform
            .pre_concat(Transform::from_row(cos, sin, -sin, cos, 0.0, 0.0));
    }

    /// Anti-aliased intersection with `rect` in local coordinates.
    pub(super) fn clip_rect(&mut self, rect: Rect) {
        let ts = self.state.transform;
        let clip = &mut self.state.clip;
        clip.mask = None;
        if is_scale_translate(&ts) {
            let device = rect.map(&ts);
            clip.rect = Some(match clip.rect {
                Some(current) => current.intersect(&device),
                None => device,
            });
            return;
        }
        let width = self.pixmap.width();
        let height = self.pixmap.height();
        let Some(mut mask) = clip
            .path_mask
            .as_deref()
            .cloned()
            .or_else(|| full_mask(width, height))
        else {
            return;
        };
        if let Some(r) = rect.to_tiny() {
            mask.intersect_path(&PathBuilder::from_rect(r), FillRule::Winding, true, ts);
        } else {
            mask.clear();
        }
        clip.path_mask = Some(Arc::new(mask));
    }

    fn clip_mask(&mut self) -> Option<Arc<Mask>> {
        let clip = &mut self.state.clip;
        if clip.rect.is_none() && clip.path_mask.is_none() {
            return None;
        }
        if let Some(mask) = &clip.mask {
            return Some(Arc::clone(mask));
        }
        let (width, height) = (self.pixmap.width(), self.pixmap.height());
        let mut mask = match &clip.path_mask {
            Some(mask) => (**mask).clone(),
            None => full_mask(width, height)?,
        };
        if let Some(rect) = clip.rect {
            match rect.to_tiny() {
                Some(r) => mask.intersect_path(
                    &PathBuilder::from_rect(r),
                    FillRule::Winding,
                    true,
                    Transform::identity(),
                ),
                None => mask.clear(),
            }
        }
        let mask = Arc::new(mask);
        clip.mask = Some(Arc::clone(&mask));
        Some(mask)
    }

    fn clip_is_empty(&self) -> bool {
        self.state.clip.rect.is_some_and(|rect| rect.is_empty())
    }

    pub(super) fn draw_rect(&mut self, rect: Rect, paint: &Paint) {
        if paint.style == PaintStyle::Fill && is_scale_translate(&self.state.transform) {
            let device = rect.map(&self.state.transform);
            self.fill_device_rect(device, paint);
            return;
        }
        if let Some(r) = rect.to_tiny() {
            self.draw_path(&PathBuilder::from_rect(r), paint);
        }
    }

    fn fill_device_rect(&mut self, rect: Rect, paint: &Paint) {
        if self.clip_is_empty() {
            return;
        }
        // Thin rects (grid lines) are cheaper in a direct loop; wide ones go through
        // tiny-skia's SIMD pipeline. Both produce identical pixels.
        if paint.shader.is_none()
            && self.state.clip.path_mask.is_none()
            && rect.width().min(rect.height()) <= THIN_RECT
        {
            self.blend_rect_solid(rect, premultiplied(paint.color));
            return;
        }
        let Some(tiny_paint) = paint.to_tiny() else {
            return;
        };
        let Some(device_rect) = rect.to_tiny() else {
            return;
        };
        // Shaders are specified in local space; express them in device space.
        let mut tiny_paint = tiny_paint;
        tiny_paint.shader.transform(self.state.transform);
        let mask = self.clip_mask();
        self.pixmap.fill_rect(
            device_rect,
            &tiny_paint,
            Transform::identity(),
            mask.as_deref(),
        );
    }

    /// Source-over fill of a device rect with exact area coverage at its edges (what
    /// tiny-skia's `fill_rect_aa` and Skia's analytic AA compute), multiplied by the
    /// clip rect's coverage. A direct loop: grid lines and lane backgrounds are the
    /// most common draws, and tiny-skia's pipeline is slow on 1-2 px wide spans.
    fn blend_rect_solid(&mut self, rect: Rect, src: [u32; 4]) {
        let (width, height) = (self.pixmap.width() as i64, self.pixmap.height() as i64);
        let clip = self.state.clip.rect;
        let area = clip.map_or(rect, |clip| rect.intersect(&clip));
        let x0 = (area.left.floor() as i64).max(0);
        let x1 = (area.right.ceil() as i64).min(width);
        let y0 = (area.top.floor() as i64).max(0);
        let y1 = (area.bottom.ceil() as i64).min(height);
        if x0 >= x1 || y0 >= y1 || src[3] == 0 {
            return;
        }
        let axis = |p: i64, lo: f32, hi: f32, clip_lo: Option<f32>, clip_hi: Option<f32>| {
            let cov = span_coverage(p, lo, hi);
            match (clip_lo, clip_hi) {
                (Some(a), Some(b)) => div255(cov * span_coverage(p, a, b)),
                _ => cov,
            }
        };
        let col_cov: Vec<u32> = (x0..x1)
            .map(|x| {
                axis(
                    x,
                    rect.left,
                    rect.right,
                    clip.map(|c| c.left),
                    clip.map(|c| c.right),
                )
            })
            .collect();
        let data = self.pixmap.data_mut();
        let opaque = src[3] == 255;
        let solid = src.map(|v| v as u8);
        for y in y0..y1 {
            let row_cov = axis(
                y,
                rect.top,
                rect.bottom,
                clip.map(|c| c.top),
                clip.map(|c| c.bottom),
            );
            if row_cov == 0 {
                continue;
            }
            let row = &mut data[(y * width) as usize * 4..][..width as usize * 4];
            for (i, x) in (x0..x1).enumerate() {
                let cov = if row_cov == 255 {
                    col_cov[i]
                } else {
                    div255(row_cov * col_cov[i])
                };
                if cov == 0 {
                    continue;
                }
                let px = &mut row[x as usize * 4..][..4];
                if cov == 255 && opaque {
                    px.copy_from_slice(&solid);
                    continue;
                }
                let s = src.map(|v| div255(v * cov));
                let inv = 255 - s[3];
                for c in 0..4 {
                    px[c] = (s[c] + div255(u32::from(px[c]) * inv)).min(255) as u8;
                }
            }
        }
    }

    pub(super) fn draw_line(&mut self, p0: (f32, f32), p1: (f32, f32), paint: &Paint) {
        let ts = self.state.transform;
        let axis_aligned = p0.0 == p1.0 || p0.1 == p1.1;
        if paint.anti_alias && axis_aligned && is_scale_translate(&ts) && paint.shader.is_none() {
            // A butt-capped stroke of an axis-aligned line is a rect. Skia fills it with
            // exact (analytic) coverage once it is wider than a hairline.
            let half = paint.stroke_width / 2.0;
            let rect = if p0.1 == p1.1 {
                Rect::from_ltrb(p0.0, p0.1 - half, p1.0, p0.1 + half)
            } else {
                Rect::from_ltrb(p0.0 - half, p0.1, p0.0 + half, p1.1)
            };
            let device = rect.map(&ts);
            let thickness = if p0.1 == p1.1 {
                device.height()
            } else {
                device.width()
            };
            if thickness > 1.0 {
                self.fill_device_rect(device, paint);
                return;
            }
            if thickness > 0.0 && self.state.clip.path_mask.is_none() {
                // Skia draws thinner strokes as an anti-aliased hairline (a 1 px wide
                // box) with alpha scaled by the width; for an axis-aligned line that is
                // exactly a 1 px rect centred on the line.
                let center = Rect::from_ltrb(
                    (device.left + device.right) / 2.0,
                    (device.top + device.bottom) / 2.0,
                    (device.left + device.right) / 2.0,
                    (device.top + device.bottom) / 2.0,
                );
                let hairline = if p0.1 == p1.1 {
                    Rect::from_ltrb(
                        device.left,
                        center.top - 0.5,
                        device.right,
                        center.top + 0.5,
                    )
                } else {
                    Rect::from_ltrb(
                        center.left - 0.5,
                        device.top,
                        center.left + 0.5,
                        device.bottom,
                    )
                };
                let mut color = paint.color;
                color.apply_opacity(thickness);
                self.blend_rect_solid(hairline, premultiplied(color));
                return;
            }
        }
        let mut builder = PathBuilder::new();
        builder.move_to(p0.0, p0.1);
        builder.line_to(p1.0, p1.1);
        if let Some(path) = builder.finish() {
            self.draw_path(&path, paint);
        }
    }

    pub(super) fn draw_round_rect(&mut self, rect: Rect, rx: f32, ry: f32, paint: &Paint) {
        if let Some(path) = round_rect_path(rect, rx, ry) {
            self.draw_path(&path, paint);
        }
    }

    pub(super) fn draw_path(&mut self, path: &Path, paint: &Paint) {
        if self.clip_is_empty() {
            return;
        }
        let ts = self.state.transform;
        let outline = match paint.style {
            PaintStyle::Fill => None,
            PaintStyle::Stroke => {
                // Hairlines (device width <= 1) keep tiny-skia's anti-aliased hairline,
                // which is a port of Skia's; thicker strokes become outlines and are
                // filled like any other path, as Skia does.
                let (sx, sy) = ts.get_scale();
                if paint.stroke_width * sx.max(sy) <= 1.0 {
                    let Some(tiny_paint) = paint.to_tiny() else {
                        return;
                    };
                    let mask = self.clip_mask();
                    self.pixmap.stroke_path(
                        path,
                        &tiny_paint,
                        &paint.stroke(),
                        ts,
                        mask.as_deref(),
                    );
                    return;
                }
                let res_scale = tiny_skia::PathStroker::compute_resolution_scale(&ts);
                let Some(outline) = path.stroke(&paint.stroke(), res_scale) else {
                    return;
                };
                Some(outline)
            }
        };
        let local = outline.as_ref().unwrap_or(path);
        let Some(device) = local.clone().transform(ts) else {
            return;
        };
        let source = match paint.shader {
            Some(gradient) => {
                let Some(inverse) = ts.invert() else {
                    return;
                };
                Source::Linear {
                    gradient,
                    device_to_local: inverse,
                }
            }
            None => Source::Solid(premultiplied(paint.color)),
        };
        self.fill_device_coverage(&device, &source);
    }

    /// Blits glyph masks the way Skia blits A8 glyph masks: coverage is
    /// remapped through `coverage_lut` (Skia's gamma/contrast pre-blend) and
    /// then blended with `color`, one glyph after another.
    pub(super) fn draw_glyphs(
        &mut self,
        glyphs: &[PlacedGlyph],
        color: Color,
        coverage_lut: &[u8; 256],
    ) {
        if self.clip_is_empty() || glyphs.is_empty() {
            return;
        }
        let src = premultiplied(color);
        let (width, height) = (self.pixmap.width() as i64, self.pixmap.height() as i64);
        let clip_rect = self.state.clip.rect;
        let path_mask = self.state.clip.path_mask.clone();
        let row_bytes = width as usize * 4;
        let mut coverage = Vec::new();
        for glyph in glyphs {
            let mask = &glyph.mask;
            let gx = glyph.x + i64::from(mask.left);
            let gy = glyph.y + i64::from(mask.top);
            let mut x0 = gx.max(0);
            let mut y0 = gy.max(0);
            let mut x1 = (gx + mask.width as i64).min(width);
            let mut y1 = (gy + mask.height as i64).min(height);
            if let Some(clip) = clip_rect {
                x0 = x0.max(clip.left.floor() as i64);
                y0 = y0.max(clip.top.floor() as i64);
                x1 = x1.min(clip.right.ceil() as i64);
                y1 = y1.min(clip.bottom.ceil() as i64);
            }
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let span = (x1 - x0) as usize;
            let data = self.pixmap.data_mut();
            for y in y0..y1 {
                let row_clip =
                    clip_rect.map_or(255, |clip| span_coverage(y, clip.top, clip.bottom));
                if row_clip == 0 {
                    continue;
                }
                let mask_row =
                    &mask.coverage[(y - gy) as usize * mask.width + (x0 - gx) as usize..][..span];
                coverage.clear();
                coverage.extend(mask_row.iter().map(|&c| coverage_lut[c as usize]));
                if let Some(clip) = clip_rect {
                    for (i, c) in coverage.iter_mut().enumerate() {
                        let col = span_coverage(x0 + i as i64, clip.left, clip.right);
                        if row_clip != 255 || col != 255 {
                            *c = div255(u32::from(*c) * div255(row_clip * col)) as u8;
                        }
                    }
                }
                if let Some(path_mask) = &path_mask {
                    let m = &path_mask.data()[(y * width + x0) as usize..][..span];
                    for (c, &m) in coverage.iter_mut().zip(m) {
                        *c = div255(u32::from(*c) * u32::from(m)) as u8;
                    }
                }
                let dst = &mut data[y as usize * row_bytes + x0 as usize * 4..][..span * 4];
                blend_solid_span(dst, &coverage, src);
            }
        }
    }

    /// Anti-aliased source-over fill of a device-space path (non-zero winding).
    ///
    /// Coverage is the exact pixel area inside the path (see `raster`), like
    /// Skia's analytic AA, instead of tiny-skia's 4x4 supersampling, which only
    /// has a few coverage levels on near-vertical edges. The outline is the one
    /// Skia's scan converter builds (see `aaa`), not the ideal one.
    fn fill_device_coverage(&mut self, path: &Path, source: &Source) {
        let (width, height) = (self.pixmap.width() as i64, self.pixmap.height() as i64);
        let bounds = path.bounds();
        let mut x0 = (bounds.left().floor() as i64).max(0);
        let mut y0 = (bounds.top().floor() as i64).max(0);
        let mut x1 = (bounds.right().ceil() as i64).min(width);
        let mut y1 = (bounds.bottom().ceil() as i64).min(height);
        let clip_rect = self.state.clip.rect;
        if let Some(clip) = clip_rect {
            x0 = x0.max(clip.left.floor() as i64);
            y0 = y0.max(clip.top.floor() as i64);
            x1 = x1.min(clip.right.ceil() as i64);
            y1 = y1.min(clip.bottom.ceil() as i64);
        }
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let (mw, mh) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut raster = Rasterizer::new(mw, mh);
        // The device clip Skia scan-converts against: the surface, narrowed to
        // the (rounded-out) clip rect.
        let mut device_clip = aaa::ClipRect {
            left: 0.0,
            top: 0.0,
            right: width as f32,
            bottom: height as f32,
        };
        if let Some(clip) = clip_rect {
            device_clip.left = device_clip.left.max(clip.left.floor());
            device_clip.top = device_clip.top.max(clip.top.floor());
            device_clip.right = device_clip.right.min(clip.right.ceil());
            device_clip.bottom = device_clip.bottom.min(clip.bottom.ceil());
        }
        aaa::add_path(&mut raster, path, device_clip, (x0, y0));
        // Clip-rect coverage of each column; `None` when every column is inside.
        let clip_cols: Option<Vec<u32>> = clip_rect
            .filter(|clip| clip.left > x0 as f32 || clip.right < x1 as f32)
            .map(|clip| {
                (x0..x1)
                    .map(|x| span_coverage(x, clip.left, clip.right))
                    .collect()
            });
        let path_mask = self.state.clip.path_mask.clone();
        let row_bytes = width as usize * 4;
        let data = self.pixmap.data_mut();
        let mut coverage = vec![0_u8; mw];
        for row in raster.rows() {
            raster.take_row(row, &mut coverage, Quantize::Round255);
            let y = y0 + row as i64;
            let row_clip = clip_rect.map_or(255, |clip| span_coverage(y, clip.top, clip.bottom));
            if row_clip == 0 {
                continue;
            }
            if row_clip != 255 || clip_cols.is_some() {
                for (i, c) in coverage.iter_mut().enumerate() {
                    let col = clip_cols.as_ref().map_or(255, |cols| cols[i]);
                    *c = div255(u32::from(*c) * div255(row_clip * col)) as u8;
                }
            }
            if let Some(path_mask) = &path_mask {
                let mask_row = &path_mask.data()[(y * width + x0) as usize..][..mw];
                for (c, &m) in coverage.iter_mut().zip(mask_row) {
                    *c = div255(u32::from(*c) * u32::from(m)) as u8;
                }
            }
            let dst = &mut data[y as usize * row_bytes + x0 as usize * 4..][..mw * 4];
            match source {
                Source::Solid(color) => blend_solid_span(dst, &coverage, *color),
                Source::Linear { .. } => {
                    for (i, (px, &cov)) in dst
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(&coverage)
                        .enumerate()
                    {
                        if cov != 0 {
                            blend_pixel(px, source.color_at(x0 + i as i64, y), u32::from(cov));
                        }
                    }
                }
            }
        }
    }

    /// `drawImageRect` with bilinear sampling, an anti-aliased destination rect and
    /// the "fast" src-rect constraint (samples may come from outside `src`).
    pub(super) fn draw_image_rect(&mut self, image: &Pixmap, src: Option<Rect>, dst: Rect) {
        if self.clip_is_empty() || dst.is_empty() {
            return;
        }
        let src = src.unwrap_or_else(|| {
            Rect::from_xywh(0.0, 0.0, image.width() as f32, image.height() as f32)
        });
        if src.is_empty() {
            return;
        }
        let sx = dst.width() / src.width();
        let sy = dst.height() / src.height();
        let image_to_local = Transform::from_row(
            sx,
            0.0,
            0.0,
            sy,
            dst.left - src.left * sx,
            dst.top - src.top * sy,
        );
        let ts = self.state.transform;
        let image_to_device = ts.pre_concat(image_to_local);

        if is_scale_translate(&ts) && self.state.clip.path_mask.is_none() {
            let mut device = dst.map(&ts);
            if let Some(clip) = self.state.clip.rect {
                device = device.intersect(&clip);
            }
            if device.is_empty() {
                return;
            }
            if image_to_device.sx == 1.0 && image_to_device.sy == 1.0 {
                blit_translated(
                    &mut self.pixmap,
                    image,
                    image_to_device.tx,
                    image_to_device.ty,
                    device,
                );
                return;
            }
            let paint = tiny_skia::Paint {
                shader: Pattern::new(
                    image.as_ref(),
                    SpreadMode::Pad,
                    FilterQuality::Bilinear,
                    1.0,
                    image_to_device,
                ),
                anti_alias: true,
                ..tiny_skia::Paint::default()
            };
            if let Some(rect) = device.to_tiny() {
                self.pixmap
                    .fill_rect(rect, &paint, Transform::identity(), None);
            }
            return;
        }

        let paint = tiny_skia::Paint {
            shader: Pattern::new(
                image.as_ref(),
                SpreadMode::Pad,
                FilterQuality::Bilinear,
                1.0,
                image_to_local,
            ),
            anti_alias: true,
            ..tiny_skia::Paint::default()
        };
        let mask = self.clip_mask();
        if let Some(rect) = dst.to_tiny() {
            self.pixmap.fill_path(
                &PathBuilder::from_rect(rect),
                &paint,
                FillRule::Winding,
                ts,
                mask.as_deref(),
            );
        }
    }
}

/// Rects at most this thick (device px) are blended by `Canvas::blend_rect_solid`.
const THIN_RECT: f32 = 4.0;

/// Source-over blend of a solid premultiplied colour through a coverage span.
fn blend_solid_span(dst: &mut [u8], coverage: &[u8], src: [u32; 4]) {
    let solid = src.map(|v| v as u8);
    let opaque = src[3] == 255;
    for (px, &cov) in dst.as_chunks_mut::<4>().0.iter_mut().zip(coverage) {
        match cov {
            0 => {}
            255 if opaque => *px = solid,
            cov => blend_pixel(px, src, u32::from(cov)),
        }
    }
}

/// Source-over of `src` (premultiplied) scaled by `cov` onto `px`.
#[inline]
fn blend_pixel(px: &mut [u8; 4], src: [u32; 4], cov: u32) {
    let s = src.map(|v| div255(v * cov));
    let inv = 255 - s[3];
    for c in 0..4 {
        px[c] = (s[c] + div255(u32::from(px[c]) * inv)).min(255) as u8;
    }
}

fn premultiplied(color: Color) -> [u32; 4] {
    let c = color.premultiply().to_color_u8();
    [c.red(), c.green(), c.blue(), c.alpha()].map(u32::from)
}

/// The paint source of a coverage fill.
enum Source {
    Solid([u32; 4]),
    /// Two-stop linear gradient, clamped, interpolated unpremultiplied (Skia's
    /// default), evaluated at pixel centres.
    Linear {
        gradient: LinearShader,
        device_to_local: Transform,
    },
}

impl Source {
    fn color_at(&self, x: i64, y: i64) -> [u32; 4] {
        match self {
            Source::Solid(color) => *color,
            Source::Linear {
                gradient,
                device_to_local,
            } => {
                let mut p = Point::from_xy(x as f32 + 0.5, y as f32 + 0.5);
                device_to_local.map_point(&mut p);
                let (dx, dy) = (
                    gradient.end.0 - gradient.start.0,
                    gradient.end.1 - gradient.start.1,
                );
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 {
                    (((p.x - gradient.start.0) * dx + (p.y - gradient.start.1) * dy) / len2)
                        .clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let [c0, c1] = gradient.colors;
                let lerp = |a: f32, b: f32| a + (b - a) * t;
                let color = Color::from_rgba(
                    lerp(c0.red(), c1.red()),
                    lerp(c0.green(), c1.green()),
                    lerp(c0.blue(), c1.blue()),
                    lerp(c0.alpha(), c1.alpha()),
                )
                .unwrap_or(c1);
                premultiplied(color)
            }
        }
    }
}

fn is_scale_translate(ts: &Transform) -> bool {
    ts.kx == 0.0 && ts.ky == 0.0
}

fn full_mask(width: u32, height: u32) -> Option<Mask> {
    let mut mask = Mask::new(width, height)?;
    mask.data_mut().fill(255);
    Some(mask)
}

/// A round rect built from cubic quarter arcs (Skia uses conics; the two differ
/// by well under 0.1% of the radius).
fn round_rect_path(rect: Rect, rx: f32, ry: f32) -> Option<Path> {
    if rect.is_empty() {
        return None;
    }
    let rx = rx.min(rect.width() / 2.0).max(0.0);
    let ry = ry.min(rect.height() / 2.0).max(0.0);
    if rx == 0.0 || ry == 0.0 {
        return Some(PathBuilder::from_rect(rect.to_tiny()?));
    }
    const KAPPA: f32 = 0.552_284_8;
    let (l, t, r, b) = (rect.left, rect.top, rect.right, rect.bottom);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let mut p = PathBuilder::new();
    p.move_to(l + rx, t);
    p.line_to(r - rx, t);
    p.cubic_to(r - rx + kx, t, r, t + ry - ky, r, t + ry);
    p.line_to(r, b - ry);
    p.cubic_to(r, b - ry + ky, r - rx + kx, b, r - rx, b);
    p.line_to(l + rx, b);
    p.cubic_to(l + rx - kx, b, l, b - ry + ky, l, b - ry);
    p.line_to(l, t + ry);
    p.cubic_to(l, t + ry - ky, l + rx - kx, t, l + rx, t);
    p.close();
    p.finish()
}

/// Source-over draw of `image` translated by a (possibly fractional) device offset,
/// bilinearly resampled with clamped edges, covering `device` with anti-aliased
/// edges. Equivalent to a bilinear image shader under a translate-only matrix.
///
/// This is the page composite of the segment rasters (~14 Mpx on a large chart),
/// so rows are processed in parallel bands and the common case (integer x offset,
/// opaque source, fully covered pixel) is a vectorizable two-row lerp.
fn blit_translated(dst: &mut PixmapMut<'_>, image: &Pixmap, tx: f32, ty: f32, device: Rect) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let (sw, sh) = (image.width() as i64, image.height() as i64);
    if sw == 0 || sh == 0 {
        return;
    }
    let x0 = (device.left.floor() as i64).max(0);
    let x1 = (device.right.ceil() as i64).min(dw);
    let y0 = (device.top.floor() as i64).max(0);
    let y1 = (device.bottom.ceil() as i64).min(dh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    // Pixel centres map to source coordinate (px - tx); bilinear taps sit at
    // floor(u) and floor(u) + 1 with a weight that is constant for the whole draw.
    let ox = f64::from(-tx).floor();
    let oy = f64::from(-ty).floor();
    let blit = TranslatedBlit {
        src: image.data(),
        sw,
        sh,
        x0,
        x1,
        ox: ox as i64,
        oy: oy as i64,
        wx: ((f64::from(-tx) - ox) * 256.0).round() as u32,
        wy: ((f64::from(-ty) - oy) * 256.0).round() as u32,
        col_cov: (x0..x1)
            .map(|x| span_coverage(x, device.left, device.right))
            .collect(),
        device,
    };

    let row_bytes = dw as usize * 4;
    let rows = &mut dst.data_mut()[y0 as usize * row_bytes..y1 as usize * row_bytes];
    let pixels = ((x1 - x0) * (y1 - y0)) as usize;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min((pixels / (128 * 1024)).max(1));
    if threads <= 1 {
        for (i, row) in rows.chunks_exact_mut(row_bytes).enumerate() {
            blit.row(row, y0 + i as i64);
        }
        return;
    }
    let band = ((y1 - y0) as usize).div_ceil(threads);
    std::thread::scope(|scope| {
        for (b, chunk) in rows.chunks_mut(band * row_bytes).enumerate() {
            let blit = &blit;
            scope.spawn(move || {
                for (i, row) in chunk.chunks_exact_mut(row_bytes).enumerate() {
                    blit.row(row, y0 + (b * band + i) as i64);
                }
            });
        }
    });
}

struct TranslatedBlit<'a> {
    src: &'a [u8],
    sw: i64,
    sh: i64,
    x0: i64,
    x1: i64,
    ox: i64,
    oy: i64,
    wx: u32,
    wy: u32,
    col_cov: Vec<u32>,
    device: Rect,
}

impl TranslatedBlit<'_> {
    fn src_row(&self, y: i64) -> &[u8] {
        let y = y.clamp(0, self.sh - 1) as usize;
        let stride = self.sw as usize * 4;
        &self.src[y * stride..(y + 1) * stride]
    }

    fn row(&self, dst: &mut [u8], y: i64) {
        let row_cov = span_coverage(y, self.device.top, self.device.bottom);
        if row_cov == 0 {
            return;
        }
        let sy = y + self.oy;
        let (r0, r1) = (self.src_row(sy), self.src_row(sy + 1));
        let wy = self.wy;
        let lerp = |a: u32, b: u32, w: u32| -> u32 { (a * (256 - w) + b * w + 128) >> 8 };

        // Fast path: integer x offset, so each output pixel is a vertical lerp of
        // one source column, computed for the whole in-bounds span at once.
        if self.wx == 0 && row_cov == 255 {
            let lo = self.x0.max(-self.ox);
            let hi = self.x1.min(self.sw - self.ox);
            if lo < hi {
                let (d0, d1) = (lo as usize * 4, hi as usize * 4);
                let (s0, s1) = (((lo + self.ox) * 4) as usize, ((hi + self.ox) * 4) as usize);
                let (a, b) = (&r0[s0..s1], &r1[s0..s1]);
                let out = &mut dst[d0..d1];
                let first = (lo - self.x0) as usize;
                let opaque_full = self.col_cov[first..first + (hi - lo) as usize]
                    .iter()
                    .all(|&c| c == 255)
                    && a.as_chunks::<4>().0.iter().all(|p| p[3] == 255)
                    && (wy == 0 || b.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
                if opaque_full {
                    if wy == 0 {
                        out.copy_from_slice(a);
                    } else {
                        for ((o, &p), &q) in out.iter_mut().zip(a).zip(b) {
                            *o = lerp(u32::from(p), u32::from(q), wy) as u8;
                        }
                    }
                    self.span(dst, self.x0, lo, row_cov, sy);
                    self.span(dst, hi, self.x1, row_cov, sy);
                    return;
                }
            }
        }
        self.span(dst, self.x0, self.x1, row_cov, sy);
    }

    /// General per-pixel path for `[from, to)`.
    fn span(&self, dst: &mut [u8], from: i64, to: i64, row_cov: u32, sy: i64) {
        let (r0, r1) = (self.src_row(sy), self.src_row(sy + 1));
        let px = |row: &[u8], x: i64| -> [u32; 4] {
            let i = x.clamp(0, self.sw - 1) as usize * 4;
            [row[i], row[i + 1], row[i + 2], row[i + 3]].map(u32::from)
        };
        let lerp = |a: [u32; 4], b: [u32; 4], w: u32| -> [u32; 4] {
            if w == 0 {
                return a;
            }
            [0, 1, 2, 3].map(|c| (a[c] * (256 - w) + b[c] * w + 128) >> 8)
        };
        for x in from..to {
            let cov = div255(row_cov * self.col_cov[(x - self.x0) as usize]);
            if cov == 0 {
                continue;
            }
            let sx = x + self.ox;
            let top = lerp(px(r0, sx), px(r0, sx + 1), self.wx);
            let pixel = if self.wy == 0 {
                top
            } else {
                lerp(top, lerp(px(r1, sx), px(r1, sx + 1), self.wx), self.wy)
            };
            let o = x as usize * 4;
            if cov == 255 && pixel[3] == 255 {
                for c in 0..4 {
                    dst[o + c] = pixel[c] as u8;
                }
                continue;
            }
            let s = pixel.map(|v| div255(v * cov));
            let inv = 255 - s[3];
            for c in 0..4 {
                dst[o + c] = (s[c] + div255(u32::from(dst[o + c]) * inv)).min(255) as u8;
            }
        }
    }
}

/// Coverage (0..=255) of pixel `p` by the span `[lo, hi)` along one axis.
fn span_coverage(p: i64, lo: f32, hi: f32) -> u32 {
    let a = (p as f32).max(lo);
    let b = ((p + 1) as f32).min(hi);
    ((b - a).clamp(0.0, 1.0) * 255.0).round() as u32
}

fn div255(v: u32) -> u32 {
    let v = v + 128;
    (v + (v >> 8)) >> 8
}
