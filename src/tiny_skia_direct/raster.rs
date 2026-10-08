//! Exact-area ("signed area accumulation") scanline rasterizer.
//!
//! This is the algorithm behind FreeType's `ftgrays` and close to what Skia's
//! analytic AA computes: every line segment adds its signed area and cover to the
//! cells it crosses, and a running sum along each row turns that into coverage.
//! Coverage is the exact area of the pixel inside the outline (non-zero winding,
//! `min(|winding area|, 1)`), so near-vertical edges get all 256 levels instead
//! of the few levels a 4x4 supersampler produces.
//!
//! What reaches it decides how close it is to a given renderer: `aaa` feeds it
//! the edges Skia's analytic AA builds, `add_path_freetype` the lines FreeType's
//! `ftgrays` draws for a glyph. `add_path` (tests only) flattens the ideal
//! outline finely.

use std::cell::RefCell;

#[cfg(test)]
use tiny_skia::Transform;
use tiny_skia::{Path, PathSegment, Point};

/// Maximum distance between a flattened curve and the true curve, in pixels.
#[cfg(test)]
const FLATTEN_TOLERANCE: f32 = 1.0 / 32.0;
/// Upper bound on the number of lines a single curve is split into.
#[cfg(test)]
const MAX_CURVE_LINES: u32 = 512;

/// How exact area maps to 8-bit coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Quantize {
    /// `round(area * 255)`: Skia's analytic AA for paths.
    Round255,
    /// `min(floor(area * 256), 255)`: FreeType's `gray_hline`, used for glyph
    /// masks rendered by FreeType.
    FreeType,
}

/// A coverage accumulator over an integer pixel box.
pub(super) struct Rasterizer {
    width: usize,
    height: usize,
    /// Row-major cells, `stride = width + 2` so that edges clamped to the right
    /// border (x = width) and their `x + 1` neighbour stay in the row.
    cells: Vec<f32>,
    /// Rows `[min_row, max_row)` that received any area.
    min_row: usize,
    max_row: usize,
}

thread_local! {
    static CELLS: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

impl Rasterizer {
    /// A rasterizer for a `width` x `height` box, reusing this thread's cell
    /// buffer (it is returned on drop).
    pub(super) fn new(width: usize, height: usize) -> Self {
        let len = (width + 2) * height;
        let mut cells = CELLS.with(|cells| std::mem::take(&mut *cells.borrow_mut()));
        cells.clear();
        cells.resize(len, 0.0);
        Self {
            width,
            height,
            cells,
            min_row: height,
            max_row: 0,
        }
    }

    #[cfg(test)]
    pub(super) fn width(&self) -> usize {
        self.width
    }

    #[cfg(test)]
    pub(super) fn height(&self) -> usize {
        self.height
    }

    /// Rows that may have non-zero coverage.
    pub(super) fn rows(&self) -> std::ops::Range<usize> {
        self.min_row..self.max_row.max(self.min_row)
    }

    /// Adds every contour of `path`, mapped by `ts` (box coordinates: the box's
    /// top-left pixel corner is the origin). Open contours are closed.
    #[cfg(test)]
    pub(super) fn add_path(&mut self, path: &Path, ts: Transform) {
        let map = |p: Point| {
            let mut p = p;
            ts.map_point(&mut p);
            p
        };
        let mut start = Point::zero();
        let mut last = Point::zero();
        let mut open = false;
        for segment in path.segments() {
            match segment {
                PathSegment::MoveTo(p) => {
                    if open {
                        self.line(last, start);
                    }
                    start = map(p);
                    last = start;
                    open = true;
                }
                PathSegment::LineTo(p) => {
                    let p = map(p);
                    self.line(last, p);
                    last = p;
                }
                PathSegment::QuadTo(p1, p2) => {
                    let (p1, p2) = (map(p1), map(p2));
                    self.quad(last, p1, p2);
                    last = p2;
                }
                PathSegment::CubicTo(p1, p2, p3) => {
                    let (p1, p2, p3) = (map(p1), map(p2), map(p3));
                    self.cubic(last, p1, p2, p3);
                    last = p3;
                }
                PathSegment::Close => {
                    if open {
                        self.line(last, start);
                    }
                    last = start;
                    open = false;
                }
            }
        }
        if open {
            self.line(last, start);
        }
    }

    /// Adds a glyph outline (device space) the way FreeType's `ftgrays` sees
    /// it, translated by `-origin` (integral): points are rounded to 26.6, then
    /// curves are split by its integer bisection (`gray_render_conic` /
    /// `gray_render_cubic`) at 1/256 px.
    pub(super) fn add_path_freetype(&mut self, path: &Path, origin: (f32, f32)) {
        // FreeType works at 1/256 px (26.6 outline points upscaled by 4).
        let up = |p: Point| -> (i64, i64) {
            (
                ((p.x - origin.0) * 64.0).round() as i64 * 4,
                ((p.y - origin.1) * 64.0).round() as i64 * 4,
            )
        };
        let to_point = |v: (i64, i64)| Point::from_xy(v.0 as f32 / 256.0, v.1 as f32 / 256.0);
        let mut start = (0, 0);
        let mut last = (0, 0);
        let mut open = false;
        for segment in path.segments() {
            match segment {
                PathSegment::MoveTo(p) => {
                    if open {
                        self.line(to_point(last), to_point(start));
                    }
                    start = up(p);
                    last = start;
                    open = true;
                }
                PathSegment::LineTo(p) => {
                    let p = up(p);
                    self.line(to_point(last), to_point(p));
                    last = p;
                }
                PathSegment::QuadTo(p1, p2) => {
                    let (p1, p2) = (up(p1), up(p2));
                    let mut points = Vec::new();
                    freetype_conic(last, p1, p2, &mut points);
                    for p in points {
                        self.line(to_point(last), to_point(p));
                        last = p;
                    }
                }
                PathSegment::CubicTo(p1, p2, p3) => {
                    let (p1, p2, p3) = (up(p1), up(p2), up(p3));
                    let mut points = Vec::new();
                    freetype_cubic(last, p1, p2, p3, &mut points);
                    for p in points {
                        self.line(to_point(last), to_point(p));
                        last = p;
                    }
                }
                PathSegment::Close => {
                    if open {
                        self.line(to_point(last), to_point(start));
                    }
                    last = start;
                    open = false;
                }
            }
        }
        if open {
            self.line(to_point(last), to_point(start));
        }
    }

    #[cfg(test)]
    fn quad(&mut self, p0: Point, p1: Point, p2: Point) {
        // Wang's formula for quadratics: n = sqrt(|p0 - 2p1 + p2| / (4 tol)).
        let dd = (p0.x - 2.0 * p1.x + p2.x).hypot(p0.y - 2.0 * p1.y + p2.y);
        let n = curve_lines((dd / (4.0 * FLATTEN_TOLERANCE)).sqrt());
        let mut prev = p0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let p = if i == n {
                p2
            } else {
                Point::from_xy(
                    mt * mt * p0.x + 2.0 * mt * t * p1.x + t * t * p2.x,
                    mt * mt * p0.y + 2.0 * mt * t * p1.y + t * t * p2.y,
                )
            };
            self.line(prev, p);
            prev = p;
        }
    }

    #[cfg(test)]
    fn cubic(&mut self, p0: Point, p1: Point, p2: Point, p3: Point) {
        // Wang's formula for cubics: n = sqrt(3/4 * max|second difference| / tol).
        let d1 = (p0.x - 2.0 * p1.x + p2.x).hypot(p0.y - 2.0 * p1.y + p2.y);
        let d2 = (p1.x - 2.0 * p2.x + p3.x).hypot(p1.y - 2.0 * p2.y + p3.y);
        let n = curve_lines((0.75 * d1.max(d2) / FLATTEN_TOLERANCE).sqrt());
        let mut prev = p0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let p = if i == n {
                p3
            } else {
                let (a, b, c, d) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
                Point::from_xy(
                    a * p0.x + b * p1.x + c * p2.x + d * p3.x,
                    a * p0.y + b * p1.y + c * p2.y + d * p3.y,
                )
            };
            self.line(prev, p);
            prev = p;
        }
    }

    /// Adds a line segment, splitting it where it crosses the box's left or right
    /// border. Parts left of the box keep their winding contribution by being
    /// moved onto x = 0; parts right of it cannot affect the box and land on
    /// x = width, outside the visible cells.
    pub(super) fn line(&mut self, p0: Point, p1: Point) {
        if !(p0.is_finite() && p1.is_finite()) || p0.y == p1.y {
            return;
        }
        let w = self.width as f32;
        // Parameters where the segment crosses x = 0 and x = w, in order.
        let mut ts = [2.0_f32; 2];
        for (slot, border) in [0.0, w].into_iter().enumerate() {
            if (p0.x < border) != (p1.x < border) {
                let t = (border - p0.x) / (p1.x - p0.x);
                if t > 0.0 && t < 1.0 {
                    ts[slot] = t;
                }
            }
        }
        if ts[1] < ts[0] {
            ts.swap(0, 1);
        }
        let clamp = |p: Point| Point::from_xy(p.x.clamp(0.0, w), p.y);
        let at = |t: f32| Point::from_xy(p0.x + (p1.x - p0.x) * t, p0.y + (p1.y - p0.y) * t);
        let mut from = p0;
        for t in ts {
            if t < 1.0 {
                let to = at(t);
                self.clipped_line(clamp(from), clamp(to));
                from = to;
            }
        }
        self.clipped_line(clamp(from), clamp(p1));
    }

    /// Accumulates a segment whose x lies within `[0, width]`.
    fn clipped_line(&mut self, p0: Point, p1: Point) {
        if p0.y == p1.y {
            return;
        }
        let (dir, top, bottom) = if p0.y < p1.y {
            (1.0_f32, p0, p1)
        } else {
            (-1.0_f32, p1, p0)
        };
        let h = self.height as f32;
        let y_start = top.y.max(0.0);
        let y_end = bottom.y.min(h);
        if y_start >= y_end {
            return;
        }
        let dxdy = (bottom.x - top.x) / (bottom.y - top.y);
        let max_x = self.width as f32;
        let mut x = (top.x + (y_start - top.y) * dxdy).clamp(0.0, max_x);
        let row_first = y_start as usize;
        let row_last = (y_end.ceil() as usize).min(self.height);
        self.min_row = self.min_row.min(row_first);
        self.max_row = self.max_row.max(row_last);
        let stride = self.width + 2;
        for row in row_first..row_last {
            let row_top = (row as f32).max(y_start);
            let row_bottom = ((row + 1) as f32).min(y_end);
            let dy = row_bottom - row_top;
            if dy <= 0.0 {
                continue;
            }
            let x_next = if row_bottom == y_end {
                (top.x + (y_end - top.y) * dxdy).clamp(0.0, max_x)
            } else {
                (x + dxdy * dy).clamp(0.0, max_x)
            };
            let d = dy * dir;
            let cells = &mut self.cells[row * stride..(row + 1) * stride];
            accumulate_span(cells, x, x_next, d);
            x = x_next;
        }
    }

    /// Converts row `y` to coverage in `out` (`width` bytes) and clears it.
    pub(super) fn take_row(&mut self, y: usize, out: &mut [u8], quantize: Quantize) {
        let stride = self.width + 2;
        let cells = &mut self.cells[y * stride..(y + 1) * stride];
        let mut acc = 0.0_f32;
        match quantize {
            Quantize::Round255 => {
                for (o, cell) in out.iter_mut().zip(cells.iter()) {
                    acc += *cell;
                    *o = (acc.abs().min(1.0) * 255.0 + 0.5) as u8;
                }
            }
            Quantize::FreeType => {
                for (o, cell) in out.iter_mut().zip(cells.iter()) {
                    acc += *cell;
                    *o = freetype_coverage(acc);
                }
            }
        }
        cells.fill(0.0);
    }
}

/// `gray_hline` for non-zero winding: FreeType's area is an integer in 1/512
/// units of a 1/256 level; positive areas floor, negative ones map through `~c`.
/// FreeType's y axis points up, so its sign is the opposite of ours.
#[inline]
fn freetype_coverage(acc: f32) -> u8 {
    let v = (-acc * 256.0 * 512.0).round() / 512.0;
    let c = v.floor();
    let c = if c < 0.0 { -c - 1.0 } else { c };
    c.min(255.0) as u8
}

impl Drop for Rasterizer {
    fn drop(&mut self) {
        let cells = std::mem::take(&mut self.cells);
        // Keep at most 16 MB of cells per thread around for reuse.
        if cells.capacity() <= 4 << 20 {
            CELLS.with(|slot| *slot.borrow_mut() = cells);
        }
    }
}

/// `gray_render_conic`: splits a quadratic into `2^k` lines, `k` chosen so
/// the deviation drops below 1/4 px (each bisection divides it by 4).
fn freetype_conic(p0: (i64, i64), p1: (i64, i64), p2: (i64, i64), out: &mut Vec<(i64, i64)>) {
    const ONE_PIXEL: i64 = 256;
    let dx = (p0.0 + p2.0 - 2 * p1.0).abs();
    let dy = (p0.1 + p2.1 - 2 * p1.1).abs();
    let mut d = dx.max(dy);
    let mut draw = 1;
    while d > ONE_PIXEL / 4 {
        d >>= 2;
        draw <<= 1;
    }
    // FreeType bisects with integer halving; split the same way recursively.
    fn split(a: [(i64, i64); 3], depth: u32, out: &mut Vec<(i64, i64)>) {
        if depth == 0 {
            out.push(a[2]);
            return;
        }
        let half = |u: i64, v: i64| (u + v) >> 1;
        let b1 = (half(a[0].0, a[1].0), half(a[0].1, a[1].1));
        let b3 = (half(a[1].0, a[2].0), half(a[1].1, a[2].1));
        let b2 = (half(b1.0, b3.0), half(b1.1, b3.1));
        split([a[0], b1, b2], depth - 1, out);
        split([b2, b3, a[2]], depth - 1, out);
    }
    split([p0, p1, p2], (draw as u32).trailing_zeros(), out);
}

/// `gray_render_cubic`: bisects until both control points are within 1/2 px
/// (in its trisection metric) of the chord, then draws the chord.
fn freetype_cubic(
    p0: (i64, i64),
    p1: (i64, i64),
    p2: (i64, i64),
    p3: (i64, i64),
    out: &mut Vec<(i64, i64)>,
) {
    const ONE_PIXEL: i64 = 256;
    fn flat(a: &[(i64, i64); 4]) -> bool {
        // FreeType's stack holds the arc reversed (a[0] is the end point).
        let (e, c2, c1, s) = (a[3], a[2], a[1], a[0]);
        (2 * e.0 - 3 * c2.0 + s.0).abs() <= ONE_PIXEL / 2
            && (2 * e.1 - 3 * c2.1 + s.1).abs() <= ONE_PIXEL / 2
            && (e.0 - 3 * c1.0 + 2 * s.0).abs() <= ONE_PIXEL / 2
            && (e.1 - 3 * c1.1 + 2 * s.1).abs() <= ONE_PIXEL / 2
    }
    fn go(a: [(i64, i64); 4], depth: u32, out: &mut Vec<(i64, i64)>) {
        if depth >= 16 || flat(&a) {
            out.push(a[3]);
            return;
        }
        // `gray_split_cubic` on (start, c1, c2, end).
        let [s, c1, c2, e] = a;
        let split = |s: i64, c1: i64, c2: i64, e: i64| {
            let mut a = s + c1;
            let b = c1 + c2;
            let mut c = c2 + e;
            let r2 = c >> 1;
            c += b;
            let r1 = c >> 2;
            let l1 = a >> 1;
            a += b;
            let l2 = a >> 2;
            let m = (a + c) >> 3;
            (l1, l2, m, r1, r2)
        };
        let x = split(s.0, c1.0, c2.0, e.0);
        let y = split(s.1, c1.1, c2.1, e.1);
        go([s, (x.0, y.0), (x.1, y.1), (x.2, y.2)], depth + 1, out);
        go([(x.2, y.2), (x.3, y.3), (x.4, y.4), e], depth + 1, out);
    }
    go([p0, p1, p2, p3], 0, out);
}

#[cfg(test)]
fn curve_lines(n: f32) -> u32 {
    if n.is_finite() {
        (n.ceil() as u32).clamp(1, MAX_CURVE_LINES)
    } else {
        1
    }
}

/// Adds the area of a segment that moves from `x0` to `x1` while descending
/// `d` (signed) within one row. `cells[i]` holds the change of coverage at
/// column `i`; the running sum over a row is the coverage.
fn accumulate_span(cells: &mut [f32], x0: f32, x1: f32, d: f32) {
    let (xa, xb) = if x0 < x1 { (x0, x1) } else { (x1, x0) };
    let xa_floor = xa.floor();
    let ia = xa_floor as usize;
    let xb_ceil = xb.ceil();
    let ib = xb_ceil as usize;
    if ib <= ia + 1 {
        // Within one cell: the part of the cell right of the segment's mean x
        // is covered here; the rest carries over to the next cell.
        let xm = 0.5 * (xa + xb) - xa_floor;
        cells[ia] += d - d * xm;
        cells[ia + 1] += d * xm;
        return;
    }
    // Across several cells: the covered area grows linearly from xa to xb.
    let s = (xb - xa).recip();
    let fa = xa - xa_floor;
    let a0 = 0.5 * s * (1.0 - fa) * (1.0 - fa);
    let fb = xb - xb_ceil + 1.0;
    let am = 0.5 * s * fb * fb;
    cells[ia] += d * a0;
    if ib == ia + 2 {
        cells[ia + 1] += d * (1.0 - a0 - am);
    } else {
        let a1 = s * (1.5 - fa);
        cells[ia + 1] += d * (a1 - a0);
        for cell in &mut cells[ia + 2..ib - 1] {
            *cell += d * s;
        }
        let a2 = a1 + (ib - ia - 3) as f32 * s;
        cells[ib - 1] += d * (1.0 - a2 - am);
    }
    cells[ib] += d * am;
}

#[cfg(test)]
mod tests {
    use super::{Quantize, Rasterizer};
    use tiny_skia::{PathBuilder, Point, Transform};

    fn coverage(r: &mut Rasterizer, q: Quantize) -> Vec<Vec<u8>> {
        let (w, h) = (r.width(), r.height());
        (0..h)
            .map(|y| {
                let mut row = vec![0; w];
                r.take_row(y, &mut row, q);
                row
            })
            .collect()
    }

    #[test]
    fn rect_has_exact_edge_coverage() {
        let mut r = Rasterizer::new(6, 4);
        let path = PathBuilder::from_rect(tiny_skia::Rect::from_ltrb(1.25, 0.5, 4.5, 3.0).unwrap());
        r.add_path(&path, Transform::identity());
        let rows = coverage(&mut r, Quantize::Round255);
        assert_eq!(rows[0], [0, 96, 128, 128, 64, 0]);
        assert_eq!(rows[1], [0, 191, 255, 255, 128, 0]);
        assert_eq!(rows[3], [0; 6]);
    }

    #[test]
    fn slanted_edge_matches_trapezoid_area() {
        // Left edge from (1, 0) to (3, 4): pixel (1, 0) is covered right of
        // x = 1 + 0.25 on average, i.e. 0.75 minus the small triangle.
        let mut r = Rasterizer::new(5, 4);
        let mut b = PathBuilder::new();
        b.move_to(1.0, 0.0);
        b.line_to(5.0, 0.0);
        b.line_to(5.0, 4.0);
        b.line_to(3.0, 4.0);
        b.close();
        r.add_path(&b.finish().unwrap(), Transform::identity());
        let rows = coverage(&mut r, Quantize::Round255);
        // Row 0: edge goes x 1 -> 1.5, area right of it in pixel 1 = 1 - 0.25.
        assert_eq!(rows[0][1], 191);
        assert_eq!(rows[0][2], 255);
        // Row 3: edge x 2.5 -> 3, pixel 2 covered 0.25.
        assert_eq!(rows[3][2], 64);
    }

    #[test]
    fn geometry_outside_the_box_keeps_winding() {
        // A rect that starts left of and ends right of the box.
        let mut r = Rasterizer::new(4, 2);
        let path =
            PathBuilder::from_rect(tiny_skia::Rect::from_ltrb(-10.0, 0.0, 2.5, 2.0).unwrap());
        r.add_path(&path, Transform::identity());
        let rows = coverage(&mut r, Quantize::Round255);
        assert_eq!(rows[0], [255, 255, 128, 0]);
        // A diagonal crossing the left border mid-row.
        let mut r = Rasterizer::new(4, 4);
        r.line(Point::from_xy(-2.0, 0.0), Point::from_xy(2.0, 4.0));
        r.line(Point::from_xy(2.0, 4.0), Point::from_xy(8.0, 4.0));
        r.line(Point::from_xy(8.0, 4.0), Point::from_xy(8.0, 0.0));
        r.line(Point::from_xy(8.0, 0.0), Point::from_xy(-2.0, 0.0));
        let rows = coverage(&mut r, Quantize::Round255);
        assert_eq!(rows[0], [255, 255, 255, 255]);
        assert_eq!(rows[2][0], 128);
        assert_eq!(rows[3][1], 128);
    }

    #[test]
    fn freetype_quantization_follows_gray_hline() {
        // Clockwise on screen (y down) is counter-clockwise for FreeType's y-up
        // raster: negative area, so exact halves land one level lower.
        let rect = tiny_skia::Rect::from_ltrb(0.5, 0.0, 2.0, 1.0).unwrap();
        let mut r = Rasterizer::new(2, 1);
        r.add_path(&PathBuilder::from_rect(rect), Transform::identity());
        let cw = coverage(&mut r, Quantize::FreeType)[0].clone();
        let mut r = Rasterizer::new(2, 1);
        let mut b = PathBuilder::new();
        b.move_to(0.5, 0.0);
        b.line_to(0.5, 1.0);
        b.line_to(2.0, 1.0);
        b.line_to(2.0, 0.0);
        b.close();
        r.add_path(&b.finish().unwrap(), Transform::identity());
        let ccw = coverage(&mut r, Quantize::FreeType)[0].clone();
        let mut both = [cw, ccw];
        both.sort();
        assert_eq!(both, [vec![127, 255], vec![128, 255]]);
    }
}
