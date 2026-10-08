//! The edge geometry Skia's analytic AA scan converter (`SkScan_AAAPath`)
//! actually fills, fed into the exact-area [`Rasterizer`].
//!
//! Skia does not fill the ideal outline. Its edges are fixed-point: end points
//! are snapped to 1/4 px vertically, slopes are computed from x deltas
//! truncated to 1/64 px, cubics are flattened by forward differencing into at
//! most 64 lines, and for paths that are not convex the edge walker feeds the
//! walked x of each line back into the next one (`keepContinuous`), so the
//! truncation error accumulates along a curve. On a long eased slide that drift
//! reaches ~0.5 px, which is the largest single difference between an ideal
//! rasterizer and Skia on a chart. Paths that are not inside the clip are
//! first chopped like `SkEdgeClipper` does, because that changes the curves
//! the forward differencing starts from.
//!
//! The coverage itself is the exact area of these edges, which is what the
//! analytic AA approximates.

use tiny_skia::{Path, PathSegment, Point};

use super::raster::Rasterizer;

/// Fixed-point helpers (`SkFixed` is 16.16, `SkFDot6` is 26.6).
const FIXED_ONE: i32 = 1 << 16;
const QUARTER: i32 = FIXED_ONE >> 2;
const HALF: i32 = FIXED_ONE >> 1;
const MAX_COEFF_SHIFT: i32 = 6;

/// The device clip rect, in whole pixels.
#[derive(Clone, Copy, Debug)]
pub(super) struct ClipRect {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) right: f32,
    pub(super) bottom: f32,
}

/// Adds the edges Skia's analytic AA would build for `path` (device space) to
/// `raster`, whose box starts at the integral device position `origin`.
/// `clip` is the device clip; it only matters when the path is not inside it.
pub(super) fn add_path(raster: &mut Rasterizer, path: &Path, clip: ClipRect, origin: (i64, i64)) {
    let bounds = path.bounds();
    let contained = bounds.left().floor() >= clip.left
        && bounds.top().floor() >= clip.top
        && bounds.right().ceil() <= clip.right
        && bounds.bottom().ceil() <= clip.bottom;
    let mut sink = EdgeSink {
        lines: Vec::new(),
        origin: (origin.0 as f32, origin.1 as f32),
        // The convex walker re-seeds each line from the exact curve; the general
        // walker continues from where the previous line ended.
        continuous: !is_convex(path),
    };
    let clip = (!contained).then_some(clip);
    let mut start = Point::zero();
    let mut last = Point::zero();
    let mut open = false;
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                if open {
                    edge_line(&mut sink, last, start, clip);
                }
                start = p;
                last = p;
                open = true;
            }
            PathSegment::LineTo(p) => {
                edge_line(&mut sink, last, p, clip);
                last = p;
            }
            PathSegment::QuadTo(p1, p2) => {
                edge_quad(&mut sink, [last, p1, p2], clip);
                last = p2;
            }
            PathSegment::CubicTo(p1, p2, p3) => {
                edge_cubic(&mut sink, [last, p1, p2, p3], clip);
                last = p3;
            }
            PathSegment::Close => {
                if open {
                    edge_line(&mut sink, last, start, clip);
                }
                last = start;
                open = false;
            }
        }
    }
    if open {
        edge_line(&mut sink, last, start, clip);
    }
    resolve_non_zero(raster, &mut sink.lines);
}

/// An edge in box coordinates, top to bottom, with its winding direction.
#[derive(Clone, Copy, Debug)]
struct Line {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    winding: i32,
}

impl Line {
    fn x_at(&self, y: f32) -> f32 {
        self.x0 + (y - self.y0) * (self.x1 - self.x0) / (self.y1 - self.y0)
    }
}

/// Non-zero fill like the analytic walker: every edge spans whole quarter-pixel
/// strips (Skia snaps y to 1/4 px), so per strip only the edges where the
/// winding number leaves or returns to zero bound the filled intervals. Only
/// those pieces reach the area accumulator; edges inside an already filled
/// interval (overlapping contours, stroke joins, fake bold) are dropped
/// instead of being counted twice.
fn resolve_non_zero(raster: &mut Rasterizer, lines: &mut [Line]) {
    if lines.is_empty() {
        return;
    }
    lines.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let simple = lines.len() <= 2;
    if simple {
        for line in lines.iter() {
            emit(raster, line, line.y0, line.y1, line.winding);
        }
        return;
    }
    let mut active: Vec<Line> = Vec::new();
    let mut crossings: Vec<(f32, f32, i32)> = Vec::new();
    let mut next = 0;
    let mut y = lines[0].y0;
    while next < lines.len() || !active.is_empty() {
        if active.is_empty() && next < lines.len() {
            y = y.max(lines[next].y0);
        }
        while next < lines.len() && lines[next].y0 <= y {
            active.push(lines[next]);
            next += 1;
        }
        active.retain(|line| line.y1 > y);
        if active.is_empty() {
            continue;
        }
        // The strip ends at the next quarter row, or earlier at an edge event.
        let mut y_end = (y * 4.0).floor() / 4.0 + 0.25;
        if next < lines.len() {
            y_end = y_end.min(lines[next].y0.max(y));
        }
        for line in &active {
            y_end = y_end.min(line.y1);
        }
        if y_end <= y {
            y_end = y + 0.25;
        }
        // Edges that cross inside the strip change the intervals there: end the
        // strip at the first crossing (the walker steps by quarter rows and
        // re-sorts there; splitting exactly keeps the union exact).
        for _ in 0..16 {
            crossings.clear();
            crossings.extend(
                active
                    .iter()
                    .map(|line| (line.x_at(y), line.x_at(y_end), line.winding)),
            );
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut first_cross = 1.0_f32;
            for pair in crossings.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (d_top, d_bottom) = (b.0 - a.0, b.1 - a.1);
                if d_bottom < 0.0 && d_top >= 0.0 {
                    let t = d_top / (d_top - d_bottom);
                    first_cross = first_cross.min(t);
                }
            }
            let min_strip = 1.0 / 1024.0;
            if first_cross >= 0.999 || (y_end - y) * first_cross < min_strip {
                if first_cross < 1.0 {
                    // Too thin to split again: accept the strip as is.
                    y_end = (y + min_strip).min(y_end);
                    crossings.clear();
                    crossings.extend(
                        active
                            .iter()
                            .map(|line| (line.x_at(y), line.x_at(y_end), line.winding)),
                    );
                }
                break;
            }
            y_end = y + (y_end - y) * first_cross;
        }
        crossings.sort_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1)));
        let mut winding = 0;
        for &(top, bottom, w) in &crossings {
            let before = winding;
            winding += w;
            if (before == 0) != (winding == 0) {
                // Entering an interval adds coverage, leaving removes it.
                let dir = if before == 0 { 1 } else { -1 };
                let piece = Line {
                    x0: top,
                    y0: y,
                    x1: bottom,
                    y1: y_end,
                    winding: dir,
                };
                emit(raster, &piece, y, y_end, dir);
            }
        }
        y = y_end;
    }
}

fn emit(raster: &mut Rasterizer, line: &Line, y0: f32, y1: f32, winding: i32) {
    let a = Point::from_xy(line.x_at(y0), y0);
    let b = Point::from_xy(line.x_at(y1), y1);
    if winding > 0 {
        raster.line(a, b);
    } else {
        raster.line(b, a);
    }
}

struct EdgeSink {
    lines: Vec<Line>,
    origin: (f32, f32),
    continuous: bool,
}

impl EdgeSink {
    /// Emits a fixed-point line (start point, slope, end y) with its winding.
    fn line(&mut self, x0: i32, y0: i32, slope: i32, y1: i32, reversed: bool) {
        let x1 = x0.wrapping_add(fixed_mul(slope, y1 - y0));
        if y1 <= y0 {
            return;
        }
        self.lines.push(Line {
            x0: fixed_to_f32(x0) - self.origin.0,
            y0: fixed_to_f32(y0) - self.origin.1,
            x1: fixed_to_f32(x1) - self.origin.0,
            y1: fixed_to_f32(y1) - self.origin.1,
            winding: if reversed { -1 } else { 1 },
        });
    }
}

fn edge_line(sink: &mut EdgeSink, p0: Point, p1: Point, clip: Option<ClipRect>) {
    match clip {
        None => skia_line(sink, p0, p1),
        Some(clip) => {
            let mut pieces = [Point::zero(); 4];
            let n = clip_line(p0, p1, clip, &mut pieces);
            for i in 0..n {
                skia_line(sink, pieces[i], pieces[i + 1]);
            }
        }
    }
}

fn edge_quad(sink: &mut EdgeSink, pts: [Point; 3], clip: Option<ClipRect>) {
    let mut mono_y = [Point::zero(); 5];
    let count_y = chop_quad_at_extrema(pts, &mut mono_y, Axis::Y);
    for i in 0..=count_y {
        let piece = [mono_y[i * 2], mono_y[i * 2 + 1], mono_y[i * 2 + 2]];
        match clip {
            None => skia_quad(sink, piece),
            Some(clip) => {
                let mut mono_x = [Point::zero(); 5];
                let count_x = chop_quad_at_extrema(piece, &mut mono_x, Axis::X);
                for j in 0..=count_x {
                    clip_mono_quad(
                        sink,
                        [mono_x[j * 2], mono_x[j * 2 + 1], mono_x[j * 2 + 2]],
                        clip,
                    );
                }
            }
        }
    }
}

fn edge_cubic(sink: &mut EdgeSink, pts: [Point; 4], clip: Option<ClipRect>) {
    let mut mono_y = [Point::zero(); 10];
    let count_y = chop_cubic_at_extrema(pts, &mut mono_y, Axis::Y);
    for i in 0..=count_y {
        let piece = [
            mono_y[i * 3],
            mono_y[i * 3 + 1],
            mono_y[i * 3 + 2],
            mono_y[i * 3 + 3],
        ];
        match clip {
            None => skia_cubic(sink, piece),
            Some(clip) => {
                let mut mono_x = [Point::zero(); 10];
                let count_x = chop_cubic_at_extrema(piece, &mut mono_x, Axis::X);
                for j in 0..=count_x {
                    let mono = [
                        mono_x[j * 3],
                        mono_x[j * 3 + 1],
                        mono_x[j * 3 + 2],
                        mono_x[j * 3 + 3],
                    ];
                    clip_mono_cubic(sink, mono, clip);
                }
            }
        }
    }
}

/// `SkAnalyticEdge::setLine` in the analytic edge walker.
fn skia_line(sink: &mut EdgeSink, p0: Point, p1: Point) {
    let fx = |v: f32| fdot6_to_fixed(to_fdot6_x4(v)) >> 2;
    let fy = |v: f32| snap_y(fdot6_to_fixed(to_fdot6_x4(v)) >> 2);
    let (mut x0, mut y0, mut x1, mut y1) = (fx(p0.x), fy(p0.y), fx(p1.x), fy(p1.y));
    let mut reversed = false;
    if y0 > y1 {
        std::mem::swap(&mut x0, &mut x1);
        std::mem::swap(&mut y0, &mut y1);
        reversed = true;
    }
    let dy = (y1 - y0) >> 10;
    if dy == 0 {
        return;
    }
    let slope = quick_div((x1 - x0) >> 10, dy);
    sink.line(x0, y0, slope, y1, reversed);
}

/// `SkAnalyticCubicEdge::setCubic` + `updateCubic`, driven the way the edge
/// walker drives it, for a y-monotonic cubic.
fn skia_cubic(sink: &mut EdgeSink, pts: [Point; 4]) {
    let mut x = pts.map(|p| to_fdot6_x4(p.x));
    let mut y = pts.map(|p| to_fdot6_x4(p.y));
    let mut reversed = false;
    if y[0] > y[3] {
        x.reverse();
        y.reverse();
        reversed = true;
    }
    if (y[0] + 32) >> 6 == (y[3] + 32) >> 6 {
        return;
    }
    let dx = cubic_delta_from_line(x);
    let dy = cubic_delta_from_line(y);
    let shift = (diff_to_shift(dx, dy, 2) + 1).min(MAX_COEFF_SHIFT);
    let mut up_shift = 6;
    let mut down_shift = shift + up_shift - 10;
    if down_shift < 0 {
        down_shift = 0;
        up_shift = 10 - shift;
    }
    let coefficients = |v: [i32; 4]| {
        let b = (3 * (v[1] - v[0])).wrapping_shl(up_shift as u32);
        let c = (3 * (v[0] - v[1] - v[1] + v[2])).wrapping_shl(up_shift as u32);
        let d = (v[3] + 3 * (v[1] - v[2]) - v[0]).wrapping_shl(up_shift as u32);
        let start = fdot6_to_fixed(v[0]);
        let d1 = b.wrapping_add(c >> shift).wrapping_add(d >> (2 * shift));
        let d2 = c
            .wrapping_mul(2)
            .wrapping_add(d.wrapping_mul(3) >> (shift - 1));
        let d3 = d.wrapping_mul(3) >> (shift - 1);
        let last = fdot6_to_fixed(v[3]);
        // The analytic edge keeps everything at 1x (`setCubic` shifts by 2).
        [start >> 2, d1 >> 2, d2 >> 2, d3 >> 2, last >> 2]
    };
    let [mut cx, mut cdx, mut cddx, cdddx, last_x] = coefficients(x);
    let [cy, mut cdy, mut cddy, cdddy, last_y] = coefficients(y);
    let last_y = snap_y(last_y);
    let mut cy = snap_y(cy);
    let mut snapped_y = cy;
    let mut count: i32 = -(1 << shift);
    // Each iteration is one `updateCubic` call that yields one line.
    loop {
        let mut old_x = cx;
        let mut old_y = cy;
        let mut emitted = None;
        let (mut new_x, mut new_y);
        loop {
            count += 1;
            if count < 0 {
                new_x = old_x.wrapping_add(cdx >> down_shift);
                cdx = cdx.wrapping_add(cddx >> shift);
                cddx = cddx.wrapping_add(cdddx);
                new_y = old_y.wrapping_add(cdy >> down_shift);
                cdy = cdy.wrapping_add(cddy >> shift);
                cddy = cddy.wrapping_add(cdddy);
            } else {
                new_x = last_x;
                new_y = last_y;
            }
            if new_y < old_y {
                new_y = old_y;
            }
            let mut new_snapped = snap_y(new_y);
            if last_y < new_snapped {
                new_snapped = last_y;
                count = 0;
            }
            let dy = (new_snapped - snapped_y) >> 10;
            if dy != 0 {
                let slope = fdot6_div((new_x - old_x) >> 10, dy);
                emitted = Some((old_x, snapped_y, slope, new_snapped));
            }
            old_x = new_x;
            old_y = new_y;
            snapped_y = new_snapped;
            if !(count < 0 && emitted.is_none()) {
                break;
            }
        }
        cx = new_x;
        cy = new_y;
        let Some((x0, y0, slope, y1)) = emitted else {
            return;
        };
        sink.line(x0, y0, slope, y1, reversed);
        if count >= 0 {
            return;
        }
        if sink.continuous {
            // `keepContinuous`: the next line starts where the walker left this one.
            cx = walk(x0, slope, y0, y1);
        }
    }
}

/// `SkAnalyticQuadraticEdge::setQuadratic` + `updateQuadratic`, driven the
/// way the edge walker drives it, for a y-monotonic quadratic.
fn skia_quad(sink: &mut EdgeSink, pts: [Point; 3]) {
    let mut x = pts.map(|p| to_fdot6_x4(p.x));
    let mut y = pts.map(|p| to_fdot6_x4(p.y));
    let mut reversed = false;
    if y[0] > y[2] {
        x.swap(0, 2);
        y.swap(0, 2);
        reversed = true;
    }
    if (y[0] + 32) >> 6 == (y[2] + 32) >> 6 {
        return;
    }
    let dx = ((x[1] << 1) - x[0] - x[2]) >> 2;
    let dy = ((y[1] << 1) - y[0] - y[2]) >> 2;
    let shift = diff_to_shift(dx, dy, 2).clamp(1, MAX_COEFF_SHIFT);
    let curve_shift = shift - 1;
    let coefficients = |v: [i32; 3]| {
        let a = (v[0] - v[1] - v[1] + v[2]).wrapping_shl(9);
        let b = fdot6_to_fixed(v[1] - v[0]);
        let start = fdot6_to_fixed(v[0]);
        let d1 = b.wrapping_add(a >> shift);
        let d2 = a >> (shift - 1);
        let last = fdot6_to_fixed(v[2]);
        [start >> 2, d1 >> 2, d2 >> 2, last >> 2]
    };
    let [mut qx, mut qdx, qddx, last_x] = coefficients(x);
    let [qy, mut qdy, qddy, last_y] = coefficients(y);
    let mut qy = snap_y(qy);
    let last_y = snap_y(last_y);
    let mut snapped_x = qx;
    let mut snapped_y = qy;
    let mut count: i32 = 1 << shift;
    loop {
        let mut old_x = qx;
        let mut old_y = qy;
        let (mut dx, mut dy) = (qdx, qdy);
        let mut emitted = None;
        let (mut new_x, mut new_y, mut new_snapped_x, mut new_snapped_y);
        loop {
            let slope;
            count -= 1;
            if count > 0 {
                new_x = old_x.wrapping_add(dx >> curve_shift);
                new_y = old_y.wrapping_add(dy >> curve_shift);
                if (dy >> curve_shift).abs() >= FIXED_ONE * 2
                    && (i64::from(dy.abs()) << 6) > i64::from(dx.abs())
                {
                    let diff_y = (new_y - snapped_y) >> 10;
                    slope = if diff_y != 0 {
                        quick_div((new_x - snapped_x) >> 10, diff_y)
                    } else {
                        i32::MAX
                    };
                    new_snapped_y = last_y.min((new_y + HALF) & !(FIXED_ONE - 1));
                    new_snapped_x = new_x.wrapping_sub(fixed_mul(slope, new_y - new_snapped_y));
                } else {
                    new_snapped_y = last_y.min(snap_y(new_y));
                    new_snapped_x = new_x;
                    let diff_y = (new_snapped_y - snapped_y) >> 10;
                    slope = if diff_y != 0 {
                        quick_div((new_x - snapped_x) >> 10, diff_y)
                    } else {
                        i32::MAX
                    };
                }
                dx = dx.wrapping_add(qddx);
                dy = dy.wrapping_add(qddy);
            } else {
                new_x = last_x;
                new_y = last_y;
                new_snapped_x = new_x;
                new_snapped_y = new_y;
                let diff_y = (new_y - snapped_y) >> 10;
                slope = if diff_y != 0 {
                    quick_div((new_x - snapped_x) >> 10, diff_y)
                } else {
                    i32::MAX
                };
            }
            if slope < i32::MAX && (new_snapped_y - snapped_y) >> 10 != 0 {
                emitted = Some((snapped_x, snapped_y, slope, new_snapped_y));
            }
            old_x = new_x;
            old_y = new_y;
            if !(count > 0 && emitted.is_none()) {
                break;
            }
        }
        qx = new_x;
        qy = new_y;
        qdx = dx;
        qdy = dy;
        snapped_x = new_snapped_x;
        snapped_y = new_snapped_y;
        let Some((x0, y0, slope, y1)) = emitted else {
            return;
        };
        if y1 >= y0 {
            sink.line(x0, y0, slope, y1, reversed);
        } else {
            // `updateLine` swaps the ends and flips the edge's winding for good.
            reversed = !reversed;
            sink.line(new_snapped_x, y1, slope, y0, reversed);
        }
        if count <= 0 {
            return;
        }
        if sink.continuous {
            // `keepContinuous`: the next line starts where the walker left this one.
            snapped_x = walk(x0, slope, y0.min(y1), y0.max(y1));
            snapped_y = y0.max(y1);
        }
    }
}

/// `quick_div`: a / b in 16.16 through Skia's reciprocal table when it can.
fn quick_div(a: i32, b: i32) -> i32 {
    let (abs_a, abs_b) = (a.abs(), b.abs());
    if (8..1024).contains(&abs_b) && abs_a < 1 << 12 {
        // `quick_inverse`: floor(2^22 / |b|) with b's sign.
        let inverse = ((1 << 22) / abs_b) * b.signum();
        (a * inverse) >> 6
    } else {
        fdot6_div(a, b)
    }
}

/// The x the general edge walker reaches when it steps a line from `y0` to `y1`
/// (quarter, half and whole-row steps, each adding `slope >> shift`).
fn walk(x0: i32, slope: i32, y0: i32, y1: i32) -> i32 {
    let mut x = x0;
    let mut y = y0;
    while y < y1 {
        let next = y1.min((y + FIXED_ONE) & !(FIXED_ONE - 1));
        let step = next - y;
        let (step, shift) = if step & QUARTER != 0 {
            (QUARTER, 2)
        } else if step & HALF != 0 {
            (HALF, 1)
        } else {
            (step, 0)
        };
        x = x.wrapping_add(slope >> shift);
        y += step;
    }
    x
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

fn coord(p: Point, axis: Axis) -> f32 {
    match axis {
        Axis::X => p.x,
        Axis::Y => p.y,
    }
}

fn set_coord(p: &mut Point, axis: Axis, v: f32) {
    match axis {
        Axis::X => p.x = v,
        Axis::Y => p.y = v,
    }
}

/// `SkChopQuadAtYExtrema` / `SkChopQuadAtXExtrema`: monotonic pieces in
/// `out` (2n + 3 points); returns the number of chops n.
fn chop_quad_at_extrema(src: [Point; 3], out: &mut [Point; 5], axis: Axis) -> usize {
    let [a, mut b, c] = src.map(|p| coord(p, axis));
    let (ab, mut bc) = (a - b, b - c);
    if ab < 0.0 {
        bc = -bc;
    }
    if ab == 0.0 || bc < 0.0 {
        if let Some(t) = unit_divide(a - b, a - b - b + c) {
            let chopped = chop_quad_at(src, t);
            out.copy_from_slice(&chopped);
            let flat = coord(out[2], axis);
            set_coord(&mut out[1], axis, flat);
            set_coord(&mut out[3], axis, flat);
            return 1;
        }
        b = if (a - b).abs() < (b - c).abs() { a } else { c };
    }
    out[0] = src[0];
    out[1] = src[1];
    out[2] = src[2];
    set_coord(&mut out[1], axis, b);
    0
}

fn chop_quad_at(src: [Point; 3], t: f32) -> [Point; 5] {
    let mix = |a: Point, b: Point| Point::from_xy(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
    let p01 = mix(src[0], src[1]);
    let p12 = mix(src[1], src[2]);
    [src[0], p01, mix(p01, p12), p12, src[2]]
}

/// `chopMonoQuadAt`: the first root of the quadratic reaching `target`.
fn chop_mono_quad_t(c: [f32; 3], target: f32) -> Option<f32> {
    let mut roots = [0.0; 2];
    let n = find_unit_quad_roots(
        c[0] - c[1] - c[1] + c[2],
        2.0 * (c[1] - c[0]),
        c[0] - target,
        &mut roots,
    );
    (n > 0).then_some(roots[0])
}

/// `SkEdgeClipper::clipMonoQuad` for a quadratic monotonic in x and y.
fn clip_mono_quad(sink: &mut EdgeSink, src: [Point; 3], clip: ClipRect) {
    let mut pts = src;
    let mut reverse = false;
    if pts[0].y > pts[2].y {
        pts.swap(0, 2);
        reverse = true;
    }
    if pts[2].y <= clip.top || pts[0].y >= clip.bottom {
        return;
    }
    if pts[0].y < clip.top {
        match chop_mono_quad_t(pts.map(|p| p.y), clip.top) {
            Some(t) => {
                let mut tmp = chop_quad_at(pts, t);
                tmp[2].y = clip.top;
                tmp[3].y = tmp[3].y.max(clip.top);
                pts[0] = tmp[2];
                pts[1] = tmp[3];
            }
            None => pts.iter_mut().for_each(|p| p.y = p.y.max(clip.top)),
        }
    }
    if pts[2].y > clip.bottom {
        match chop_mono_quad_t(pts.map(|p| p.y), clip.bottom) {
            Some(t) => {
                let mut tmp = chop_quad_at(pts, t);
                tmp[1].y = tmp[1].y.min(clip.bottom);
                tmp[2].y = clip.bottom;
                pts[1] = tmp[1];
                pts[2] = tmp[2];
            }
            None => pts.iter_mut().for_each(|p| p.y = p.y.min(clip.bottom)),
        }
    }
    if pts[0].x > pts[2].x {
        pts.swap(0, 2);
        reverse = !reverse;
    }
    let emit_quad = |sink: &mut EdgeSink, q: [Point; 3]| {
        if reverse {
            skia_quad(sink, [q[2], q[1], q[0]]);
        } else {
            skia_quad(sink, q);
        }
    };
    let vline = |sink: &mut EdgeSink, x: f32, y0: f32, y1: f32| {
        let (y0, y1) = if reverse { (y1, y0) } else { (y0, y1) };
        skia_line(sink, Point::from_xy(x, y0), Point::from_xy(x, y1));
    };
    if pts[2].x <= clip.left {
        vline(sink, clip.left, pts[0].y, pts[2].y);
        return;
    }
    if pts[0].x >= clip.right {
        vline(sink, clip.right, pts[0].y, pts[2].y);
        return;
    }
    if pts[0].x < clip.left {
        match chop_mono_quad_t(pts.map(|p| p.x), clip.left) {
            Some(t) => {
                let mut tmp = chop_quad_at(pts, t);
                vline(sink, clip.left, tmp[0].y, tmp[2].y);
                tmp[2].x = clip.left;
                tmp[3].x = tmp[3].x.max(clip.left);
                pts[0] = tmp[2];
                pts[1] = tmp[3];
            }
            None => {
                vline(sink, clip.left, pts[0].y, pts[2].y);
                return;
            }
        }
    }
    if pts[2].x > clip.right {
        match chop_mono_quad_t(pts.map(|p| p.x), clip.right) {
            Some(t) => {
                let mut tmp = chop_quad_at(pts, t);
                tmp[1].x = tmp[1].x.min(clip.right);
                tmp[2].x = clip.right;
                emit_quad(sink, [tmp[0], tmp[1], tmp[2]]);
                vline(sink, clip.right, tmp[2].y, tmp[4].y);
            }
            None => {
                pts[1].x = pts[1].x.min(clip.right);
                pts[2].x = pts[2].x.min(clip.right);
                emit_quad(sink, pts);
            }
        }
    } else {
        emit_quad(sink, pts);
    }
}

/// `SkChopCubicAtYExtrema` / `SkChopCubicAtXExtrema`: monotonic pieces in
/// `out` (3n + 4 points); returns the number of chops n.
fn chop_cubic_at_extrema(src: [Point; 4], out: &mut [Point; 10], axis: Axis) -> usize {
    let [a, b, c, d] = src.map(|p| coord(p, axis));
    let mut roots = [0.0_f32; 2];
    let n = find_unit_quad_roots(
        d - a + 3.0 * (b - c),
        2.0 * (a - b - b + c),
        b - a,
        &mut roots,
    );
    out[..4].copy_from_slice(&src);
    let mut base = 0;
    let mut prev_t = 0.0_f32;
    for &t in &roots[..n] {
        let local = if base == 0 {
            t
        } else {
            ((t - prev_t) / (1.0 - prev_t)).clamp(0.0, 1.0)
        };
        let piece = [out[base], out[base + 1], out[base + 2], out[base + 3]];
        let chopped = chop_cubic_at(piece, local);
        out[base..base + 7].copy_from_slice(&chopped);
        base += 3;
        prev_t = t;
    }
    if n > 0 {
        // Flatten the extrema so every piece is monotonic.
        let flat = coord(out[3], axis);
        set_coord(&mut out[2], axis, flat);
        set_coord(&mut out[4], axis, flat);
        if n == 2 {
            let flat = coord(out[6], axis);
            set_coord(&mut out[5], axis, flat);
            set_coord(&mut out[7], axis, flat);
        }
    }
    n
}

/// `valid_unit_divide`: numer / denom when it lies in (0, 1).
fn unit_divide(numer: f32, denom: f32) -> Option<f32> {
    let (mut numer, mut denom) = (numer, denom);
    if numer < 0.0 {
        numer = -numer;
        denom = -denom;
    }
    if denom == 0.0 || numer == 0.0 || numer >= denom {
        return None;
    }
    let r = numer / denom;
    (r.is_finite() && r > 0.0).then_some(r)
}

/// `SkFindUnitQuadRoots`: roots of `A t^2 + B t + C` in (0, 1), sorted.
fn find_unit_quad_roots(a: f32, b: f32, c: f32, roots: &mut [f32; 2]) -> usize {
    let mut n = 0;
    if a == 0.0 {
        if let Some(r) = unit_divide(-c, b) {
            roots[0] = r;
            n = 1;
        }
        return n;
    }
    let dr = f64::from(b) * f64::from(b) - 4.0 * f64::from(a) * f64::from(c);
    if dr < 0.0 {
        return 0;
    }
    let r = dr.sqrt() as f32;
    if !r.is_finite() {
        return 0;
    }
    let q = if b < 0.0 {
        -(b - r) / 2.0
    } else {
        -(b + r) / 2.0
    };
    for (numer, denom) in [(q, a), (c, q)] {
        if let Some(root) = unit_divide(numer, denom) {
            roots[n] = root;
            n += 1;
        }
    }
    if n == 2 {
        if roots[0] > roots[1] {
            roots.swap(0, 1);
        } else if roots[0] == roots[1] {
            n = 1;
        }
    }
    n
}

/// `SkChopCubicAt` (de Casteljau, `(b - a) * t + a`).
fn chop_cubic_at(src: [Point; 4], t: f32) -> [Point; 7] {
    if t == 1.0 {
        return [src[0], src[1], src[2], src[3], src[3], src[3], src[3]];
    }
    let mix = |a: Point, b: Point| Point::from_xy((b.x - a.x) * t + a.x, (b.y - a.y) * t + a.y);
    let ab = mix(src[0], src[1]);
    let bc = mix(src[1], src[2]);
    let cd = mix(src[2], src[3]);
    let abc = mix(ab, bc);
    let bcd = mix(bc, cd);
    let abcd = mix(abc, bcd);
    [src[0], ab, abc, abcd, bcd, cd, src[3]]
}

/// `SkChopMonoCubicAtY`/`X`: chops where the cubic crosses `value` on `axis`,
/// solving in double precision.
fn chop_mono_cubic_at(src: [Point; 4], value: f32, axis: Axis) -> [Point; 7] {
    let c = src.map(|p| f64::from(coord(p, axis)));
    let eval = |t: f64| {
        let mt = 1.0 - t;
        mt * mt * mt * c[0] + 3.0 * mt * mt * t * c[1] + 3.0 * mt * t * t * c[2] + t * t * t * c[3]
    };
    let target = f64::from(value);
    let increasing = c[3] >= c[0];
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..64 {
        let mid = 0.5 * (lo + hi);
        if (eval(mid) < target) == increasing {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let t = 0.5 * (lo + hi);
    let p = src.map(|p| (f64::from(p.x), f64::from(p.y)));
    let mix = |a: (f64, f64), b: (f64, f64)| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    let ab = mix(p[0], p[1]);
    let bc = mix(p[1], p[2]);
    let cd = mix(p[2], p[3]);
    let abc = mix(ab, bc);
    let bcd = mix(bc, cd);
    let abcd = mix(abc, bcd);
    [p[0], ab, abc, abcd, bcd, cd, p[3]].map(|(x, y)| Point::from_xy(x as f32, y as f32))
}

/// `SkEdgeClipper::clipMonoCubic` for a cubic monotonic in x and y.
fn clip_mono_cubic(sink: &mut EdgeSink, src: [Point; 4], clip: ClipRect) {
    let mut pts = src;
    let mut reverse = false;
    if pts[0].y > pts[3].y {
        pts.reverse();
        reverse = true;
    }
    if pts[3].y <= clip.top || pts[0].y >= clip.bottom {
        return;
    }
    // Chop in y.
    if pts[0].y < clip.top {
        let mut tmp = chop_mono_cubic_at(pts, clip.top, Axis::Y);
        if tmp[3].y < clip.top && tmp[4].y < clip.top && tmp[5].y < clip.top {
            tmp = chop_mono_cubic_at([tmp[3], tmp[4], tmp[5], tmp[6]], clip.top, Axis::Y);
        }
        tmp[3].y = clip.top;
        tmp[4].y = tmp[4].y.max(clip.top);
        pts[0] = tmp[3];
        pts[1] = tmp[4];
        pts[2] = tmp[5];
    }
    if pts[3].y > clip.bottom {
        let mut tmp = chop_mono_cubic_at(pts, clip.bottom, Axis::Y);
        tmp[3].y = clip.bottom;
        tmp[2].y = tmp[2].y.min(clip.bottom);
        pts[1] = tmp[1];
        pts[2] = tmp[2];
        pts[3] = tmp[3];
    }
    if pts[0].x > pts[3].x {
        pts.swap(0, 3);
        pts.swap(1, 2);
        reverse = !reverse;
    }
    let emit_cubic = |sink: &mut EdgeSink, c: [Point; 4]| {
        if reverse {
            skia_cubic(sink, [c[3], c[2], c[1], c[0]]);
        } else {
            skia_cubic(sink, c);
        }
    };
    let vline = |sink: &mut EdgeSink, x: f32, y0: f32, y1: f32| {
        let (y0, y1) = if reverse { (y1, y0) } else { (y0, y1) };
        skia_line(sink, Point::from_xy(x, y0), Point::from_xy(x, y1));
    };
    if pts[3].x <= clip.left {
        vline(sink, clip.left, pts[0].y, pts[3].y);
        return;
    }
    if pts[0].x >= clip.right {
        // `canCullToTheRight` is false for analytic AA.
        vline(sink, clip.right, pts[0].y, pts[3].y);
        return;
    }
    if pts[0].x < clip.left {
        let mut tmp = chop_mono_cubic_at(pts, clip.left, Axis::X);
        vline(sink, clip.left, tmp[0].y, tmp[3].y);
        tmp[3].x = clip.left;
        tmp[4].x = tmp[4].x.max(clip.left);
        pts[0] = tmp[3];
        pts[1] = tmp[4];
        pts[2] = tmp[5];
    }
    if pts[3].x > clip.right {
        let mut tmp = chop_mono_cubic_at(pts, clip.right, Axis::X);
        tmp[3].x = clip.right;
        tmp[2].x = tmp[2].x.min(clip.right);
        emit_cubic(sink, [tmp[0], tmp[1], tmp[2], tmp[3]]);
        vline(sink, clip.right, tmp[3].y, tmp[6].y);
    } else {
        emit_cubic(sink, pts);
    }
}

/// `SkLineClipper::ClipLine`: up to 3 lines (4 points, in order) inside the
/// clip in y, with the parts outside in x turned into vertical lines.
fn clip_line(p0: Point, p1: Point, clip: ClipRect, out: &mut [Point; 4]) -> usize {
    let pts = [p0, p1];
    let (i0, i1) = if p0.y < p1.y { (0, 1) } else { (1, 0) };
    if pts[i1].y <= clip.top || pts[i0].y >= clip.bottom {
        return 0;
    }
    let sect_h = |y: f32| -> f32 {
        let (dx, dy) = (f64::from(p1.x - p0.x), f64::from(p1.y - p0.y));
        if dy == 0.0 {
            return (p0.x + p1.x) * 0.5;
        }
        (f64::from(p0.x) + f64::from(y - p0.y) * dx / dy) as f32
    };
    let mut tmp = pts;
    if pts[i0].y < clip.top {
        tmp[i0] = Point::from_xy(sect_h(clip.top), clip.top);
    }
    if tmp[i1].y > clip.bottom {
        tmp[i1] = Point::from_xy(sect_h(clip.bottom), clip.bottom);
    }
    let (j0, j1, mut reverse) = if p0.x < p1.x {
        (0, 1, false)
    } else {
        (1, 0, true)
    };
    let sect_v = |x: f32| -> f32 {
        let (dx, dy) = (
            f64::from(tmp[1].x - tmp[0].x),
            f64::from(tmp[1].y - tmp[0].y),
        );
        if dx == 0.0 {
            return (tmp[0].y + tmp[1].y) * 0.5;
        }
        let y = f64::from(tmp[0].y) + f64::from(x - tmp[0].x) * dy / dx;
        (y as f32).clamp(tmp[0].y.min(tmp[1].y), tmp[0].y.max(tmp[1].y))
    };
    let mut result = [Point::zero(); 4];
    let count;
    if tmp[j1].x <= clip.left {
        result[0] = Point::from_xy(clip.left, tmp[0].y);
        result[1] = Point::from_xy(clip.left, tmp[1].y);
        count = 1;
        reverse = false;
    } else if tmp[j0].x >= clip.right {
        result[0] = Point::from_xy(clip.right, tmp[0].y);
        result[1] = Point::from_xy(clip.right, tmp[1].y);
        count = 1;
        reverse = false;
    } else {
        let mut r = 0;
        if tmp[j0].x < clip.left {
            result[r] = Point::from_xy(clip.left, tmp[j0].y);
            r += 1;
            result[r] = Point::from_xy(clip.left, sect_v(clip.left));
        } else {
            result[r] = tmp[j0];
        }
        r += 1;
        if tmp[j1].x > clip.right {
            result[r] = Point::from_xy(clip.right, sect_v(clip.right));
            r += 1;
            result[r] = Point::from_xy(clip.right, tmp[j1].y);
        } else {
            result[r] = tmp[j1];
        }
        count = r;
    }
    if reverse {
        for i in 0..=count {
            out[count - i] = result[i];
        }
    } else {
        out[..=count].copy_from_slice(&result[..=count]);
    }
    count
}

/// `SkPathPriv::ComputeConvexity` (single contour, consistent turning, at most
/// three sign changes per axis, at most two reversals).
fn is_convex(path: &Path) -> bool {
    let points = path.points();
    if is_concave_by_sign(points) {
        return false;
    }
    let mut state = Convexicator::default();
    let mut contours = 0;
    let mut needs_close = false;
    for segment in path.segments() {
        let pts: &[Point] = match &segment {
            PathSegment::MoveTo(p) => {
                if contours == 0 {
                    state.set_move_pt(*p);
                    continue;
                }
                if contours == 1 {
                    if !state.close() {
                        return false;
                    }
                    needs_close = false;
                    contours += 1;
                }
                continue;
            }
            PathSegment::Close => {
                if contours == 1 {
                    if !state.close() {
                        return false;
                    }
                    needs_close = false;
                    contours += 1;
                }
                continue;
            }
            PathSegment::LineTo(p) => &[*p],
            PathSegment::QuadTo(p1, p2) => &[*p1, *p2],
            PathSegment::CubicTo(p1, p2, p3) => &[*p1, *p2, *p3],
        };
        if contours == 0 {
            contours = 1;
            needs_close = true;
        }
        if contours > 1 {
            return false;
        }
        for &p in pts {
            if !state.add_pt(p) {
                return false;
            }
        }
    }
    if needs_close && !state.close() {
        return false;
    }
    !(state.first_direction_unknown && state.reversals >= 3)
}

fn is_concave_by_sign(points: &[Point]) -> bool {
    if points.len() <= 3 {
        return false;
    }
    let mut dxes = 0;
    let mut dyes = 0;
    let mut last_sx = 2;
    let mut last_sy = 2;
    let mut current = points[0];
    let mut check = |next: Point| -> bool {
        let (vx, vy) = (next.x - current.x, next.y - current.y);
        if vx != 0.0 || vy != 0.0 {
            if !(vx.is_finite() && vy.is_finite()) {
                return true;
            }
            let (sx, sy) = (i32::from(vx < 0.0), i32::from(vy < 0.0));
            dxes += i32::from(sx != last_sx);
            dyes += i32::from(sy != last_sy);
            if dxes > 3 || dyes > 3 {
                return true;
            }
            last_sx = sx;
            last_sy = sy;
        }
        current = next;
        false
    };
    for &p in &points[1..] {
        if check(p) {
            return true;
        }
    }
    check(points[0])
}

#[derive(Default)]
struct Convexicator {
    first_pt: Point,
    first_vec: Point,
    last_pt: Point,
    last_vec: Point,
    /// 0 = invalid, 1 = left, 2 = right.
    expected: u8,
    first_direction_unknown: bool,
    reversals: i32,
}

impl Convexicator {
    fn set_move_pt(&mut self, p: Point) {
        self.first_pt = p;
        self.last_pt = p;
        self.expected = 0;
    }

    fn add_pt(&mut self, p: Point) -> bool {
        if self.last_pt == p {
            return true;
        }
        let vec = Point::from_xy(p.x - self.last_pt.x, p.y - self.last_pt.y);
        if self.first_pt == self.last_pt
            && self.expected == 0
            && self.last_vec.x == 0.0
            && self.last_vec.y == 0.0
        {
            self.last_vec = vec;
            self.first_vec = vec;
        } else if !self.add_vec(vec) {
            return false;
        }
        self.last_pt = p;
        true
    }

    fn close(&mut self) -> bool {
        let first = self.first_pt;
        let first_vec = self.first_vec;
        self.add_pt(first) && self.add_vec(first_vec)
    }

    fn add_vec(&mut self, vec: Point) -> bool {
        let cross = self.last_vec.x * vec.y - self.last_vec.y * vec.x;
        if !cross.is_finite() {
            return false;
        }
        if cross == 0.0 {
            if self.last_vec.x * vec.x + self.last_vec.y * vec.y < 0.0 {
                self.last_vec = vec;
                self.reversals += 1;
                return self.reversals < 3;
            }
            return true;
        }
        let dir = if cross > 0.0 { 2 } else { 1 };
        if self.expected == 0 {
            self.expected = dir;
        } else if dir != self.expected {
            self.first_direction_unknown = true;
            return false;
        }
        self.last_vec = vec;
        true
    }
}

fn to_fdot6_x4(v: f32) -> i32 {
    (v * 256.0) as i32
}

fn fdot6_to_fixed(v: i32) -> i32 {
    v.wrapping_shl(10)
}

fn fixed_to_f32(v: i32) -> f32 {
    v as f32 / 65536.0
}

fn fixed_mul(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b)) >> 16) as i32
}

/// `SkFDot6Div`: (a << 16) / b, truncated.
fn fdot6_div(a: i32, b: i32) -> i32 {
    ((i64::from(a) << 16) / i64::from(b)).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// `SnapY`: rounds a 16.16 y to a quarter pixel.
fn snap_y(y: i32) -> i32 {
    (((y as u32).wrapping_add(1 << 13)) >> 14 << 14) as i32
}

fn diff_to_shift(dx: i32, dy: i32, shift_aa: i32) -> i32 {
    let (dx, dy) = (dx.abs(), dy.abs());
    let dist = if dx > dy {
        dx + (dy >> 1)
    } else {
        dy + (dx >> 1)
    };
    let dist = (dist + (1 << (2 + shift_aa))) >> (3 + shift_aa);
    (32 - dist.leading_zeros() as i32) >> 1
}

fn cubic_delta_from_line(v: [i32; 4]) -> i32 {
    let [a, b, c, d] = v;
    let one_third = ((a * 8 - b * 15 + 6 * c + d) * 19) >> 9;
    let two_third = ((a + 6 * b - c * 15 + d * 8) * 19) >> 9;
    one_third.abs().max(two_third.abs())
}

#[cfg(test)]
mod tests {
    use super::{is_convex, snap_y, walk};
    use tiny_skia::PathBuilder;

    #[test]
    fn snaps_to_quarter_pixels() {
        assert_eq!(snap_y(0x1_2000), 0x1_4000);
        assert_eq!(snap_y(0x1_1fff), 0x1_0000);
    }

    #[test]
    fn walker_steps_quarters_then_rows() {
        // From y = 0.25 to 2.0 with slope 1.0: 0.25 + 0.5 steps, then a full row.
        assert_eq!(
            walk(0, 0x1_0000, 0x4000, 0x2_0000),
            0x4000 + 0x8000 + 0x1_0000
        );
    }

    #[test]
    fn convexity_matches_skia_for_simple_shapes() {
        let rect = PathBuilder::from_rect(tiny_skia::Rect::from_ltrb(0.0, 0.0, 4.0, 4.0).unwrap());
        assert!(is_convex(&rect));
        let mut s = PathBuilder::new();
        s.move_to(0.0, 0.0);
        s.cubic_to(0.0, 50.0, 100.0, 50.0, 100.0, 100.0);
        s.line_to(200.0, 100.0);
        s.cubic_to(200.0, 50.0, 100.0, 50.0, 100.0, 0.0);
        s.close();
        assert!(!is_convex(&s.finish().unwrap()));
    }
}
