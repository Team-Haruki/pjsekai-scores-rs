use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use skia_safe::{
    Color, Data, EncodedImageFormat, FilterMode, Font, FontMgr, FontStyle, Image, Paint,
    PaintStyle, PathBuilder, Point, Rect, SamplingOptions, Typeface, Unichar, surfaces,
};

use crate::drawing::{CoverObject, Drawing, DrawingConfig};
use crate::fraction::Fraction;
use crate::lyric::Lyric;
use crate::notes::directional::DirectionalType;
use crate::notes::event::Event;
use crate::notes::slide::SlideType;
use crate::notes::{NO_NOTE, NoteData, NoteIdx};
use crate::score::Score;

type BezierPoints = [(f64, f64); 4];
type CustomTypefaceMap = HashMap<String, Vec<Typeface>>;
type SharedCustomTypefaces = Arc<CustomTypefaceMap>;

const CUSTOM_FONT_CACHE_MAX_ENTRIES: usize = 8;

static CUSTOM_FONT_CACHE: LazyLock<Mutex<HashMap<Vec<FontFileKey>, SharedCustomTypefaces>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const FALLBACK_FONT_FAMILIES: &[&str] = &[
    "Hiragino Sans",
    "Hiragino Kaku Gothic Pro",
    "Noto Sans CJK JP",
    "Noto Sans CJK SC",
    "Noto Sans JP",
    "Source Han Sans",
    "Source Han Sans JP",
    "Source Han Sans SC",
    "WenQuanYi Zen Hei",
    "Yu Gothic",
    "Meiryo",
    "Microsoft YaHei",
    "Avenir",
    "Helvetica Neue",
    "Arial",
    "sans-serif",
];

#[derive(Debug)]
pub enum SkiaDirectError {
    InvalidSize,
    Surface,
    RenderWorker,
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Font {
        path: PathBuf,
        source: std::io::Error,
    },
    Decode(PathBuf),
    Encode,
}

#[derive(Debug, Clone, Copy)]
pub enum SkiaImageFormat {
    Png,
    Jpeg { quality: u8 },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SkiaRenderStats {
    pub layout: Duration,
    pub setup: Duration,
    pub draw: Duration,
    pub encode: Duration,
    pub copy: Duration,
    pub total: Duration,
}

#[derive(Debug)]
pub struct SkiaImageOutput {
    pub bytes: Vec<u8>,
    pub stats: SkiaRenderStats,
}

impl fmt::Display for SkiaDirectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkiaDirectError::InvalidSize => f.write_str("chart has an invalid raster size"),
            SkiaDirectError::Surface => f.write_str("failed to create Skia raster surface"),
            SkiaDirectError::RenderWorker => f.write_str("Skia segment render worker panicked"),
            SkiaDirectError::Io { path, source } => {
                write!(f, "failed to read image asset {}: {source}", path.display())
            }
            SkiaDirectError::Font { path, source } => {
                write!(f, "failed to read font {}: {source}", path.display())
            }
            SkiaDirectError::Decode(path) => {
                write!(f, "failed to decode image asset {}", path.display())
            }
            SkiaDirectError::Encode => f.write_str("failed to encode raster output"),
        }
    }
}

impl std::error::Error for SkiaDirectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SkiaDirectError::Io { source, .. } | SkiaDirectError::Font { source, .. } => {
                Some(source)
            }
            _ => None,
        }
    }
}

pub fn score_to_skia_png(
    drawing: &mut Drawing,
    score: &mut Score,
    lyric: Option<&Lyric>,
) -> Result<Vec<u8>, SkiaDirectError> {
    score_to_skia_image(drawing, score, lyric, SkiaImageFormat::Png)
}

pub fn score_to_skia_jpeg(
    drawing: &mut Drawing,
    score: &mut Score,
    lyric: Option<&Lyric>,
    quality: u8,
) -> Result<Vec<u8>, SkiaDirectError> {
    score_to_skia_image(drawing, score, lyric, SkiaImageFormat::Jpeg { quality })
}

pub fn score_to_skia_image(
    drawing: &mut Drawing,
    score: &mut Score,
    lyric: Option<&Lyric>,
    format: SkiaImageFormat,
) -> Result<Vec<u8>, SkiaDirectError> {
    Ok(score_to_skia_image_with_stats(drawing, score, lyric, format)?.bytes)
}

pub fn score_to_skia_image_with_stats(
    drawing: &mut Drawing,
    score: &mut Score,
    lyric: Option<&Lyric>,
    format: SkiaImageFormat,
) -> Result<SkiaImageOutput, SkiaDirectError> {
    let total_started = Instant::now();
    if drawing.skill {
        drawing.build_skill_covers(score);
    }

    let layout_started = Instant::now();
    let layout = Layout::new(&drawing.config, score);
    let layout_duration = layout_started.elapsed();
    let width = round_even(layout.final_width) as i32;
    let height = round_even(layout.final_height) as i32;
    if width <= 0 || height <= 0 {
        return Err(SkiaDirectError::InvalidSize);
    }

    let setup_started = Instant::now();
    let mut surface =
        surfaces::raster_n32_premul((width, height)).ok_or(SkiaDirectError::Surface)?;
    let styles = CssStyles::parse(&drawing.style_sheet);
    let renderer = DirectRenderer::new(drawing, styles)?;
    let setup_duration = setup_started.elapsed();
    let draw_started = Instant::now();
    renderer.draw_page(surface.canvas(), score, lyric, &layout)?;
    let draw_duration = draw_started.elapsed();

    let image = surface.image_snapshot();
    let (encoded_format, quality) = match format {
        SkiaImageFormat::Png => (EncodedImageFormat::PNG, 100),
        SkiaImageFormat::Jpeg { quality } => (EncodedImageFormat::JPEG, quality.min(100) as u32),
    };
    let encode_started = Instant::now();
    #[allow(deprecated)]
    let data = image
        .encode_to_data_with_quality(encoded_format, quality)
        .ok_or(SkiaDirectError::Encode)?;
    let encode_duration = encode_started.elapsed();
    let copy_started = Instant::now();
    let bytes = data.as_bytes().to_vec();
    let copy_duration = copy_started.elapsed();
    Ok(SkiaImageOutput {
        bytes,
        stats: SkiaRenderStats {
            layout: layout_duration,
            setup: setup_duration,
            draw: draw_duration,
            encode: encode_duration,
            copy: copy_duration,
            total: total_started.elapsed(),
        },
    })
}

struct Layout {
    segments: Vec<Segment>,
    total_width: f64,
    max_height: f64,
    final_width: f64,
    final_height: f64,
}

struct Segment {
    start: i32,
    stop: i32,
    width: f64,
    height: f64,
}

impl Layout {
    fn new(cfg: &DrawingConfig, score: &mut Score) -> Self {
        let n_bars = score
            .active_notes
            .last()
            .map(|&idx| score.notes[idx].bar().ceil() as i32)
            .unwrap_or(0);

        let target_pixel_height = cfg.time_height * cfg.target_segment_seconds;
        let mut ranges: Vec<(i32, i32)> = Vec::new();
        let mut bar_start = 0;
        let mut event = Event::new(Fraction::zero());
        event.bpm = Some(Fraction::from_integer(120));
        event.bar_length = Some(Fraction::from_integer(4));
        event.sentence_length = Some(4);

        for i in 0..=n_bars {
            let e = score.get_event(Fraction::from_integer(i as i64));
            let current_height = cfg.time_height
                * score.get_time_delta_f64(
                    Fraction::from_integer(bar_start as i64),
                    Fraction::from_integer(i as i64),
                );

            if bar_start != i
                && (e.section != event.section
                    || current_height >= target_pixel_height
                    || i == n_bars)
            {
                ranges.push((bar_start, i));
                bar_start = i;
            }

            event.merge_from(&e);
        }

        let mut segments = Vec::with_capacity(ranges.len());
        let mut total_width = 0.0;
        let mut max_height = 0.0;
        for (start, stop) in ranges {
            let height = cfg.time_height
                * score.get_time_delta_f64(
                    Fraction::from_integer(start as i64),
                    Fraction::from_integer(stop as i64),
                )
                + cfg.time_padding as f64 * 2.0;
            let width = cfg.lane_width as f64 * cfg.n_lanes as f64 + cfg.lane_padding as f64 * 2.0;
            total_width += width;
            if height > max_height {
                max_height = height;
            }
            segments.push(Segment {
                start,
                stop,
                width,
                height,
            });
        }

        let final_width = total_width + cfg.lane_padding as f64 * 2.0;
        let final_height = max_height
            + cfg.time_padding as f64 * 2.0
            + cfg.meta_size as f64
            + cfg.time_padding as f64 * 2.0;

        Self {
            segments,
            total_width,
            max_height,
            final_width,
            final_height,
        }
    }
}

struct DirectRenderer<'a> {
    drawing: &'a Drawing,
    styles: CssStyles,
    font_mgr: FontMgr,
    custom_typefaces: SharedCustomTypefaces,
    note_assets: NoteAssets,
    font_cache: Mutex<HashMap<FontKey, Font>>,
}

impl<'a> DirectRenderer<'a> {
    fn new(drawing: &'a Drawing, styles: CssStyles) -> Result<Self, SkiaDirectError> {
        let note_assets = NoteAssets::load(&drawing.config);
        let (font_mgr, custom_typefaces) = build_font_manager(&drawing.config)?;
        Ok(Self {
            drawing,
            styles,
            font_mgr,
            custom_typefaces,
            note_assets,
            font_cache: Mutex::new(HashMap::new()),
        })
    }

    fn draw_page(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        lyric: Option<&Lyric>,
        layout: &Layout,
    ) -> Result<(), SkiaDirectError> {
        let cfg = &self.drawing.config;
        canvas.clear(self.color("background", RGBA::WHITE).to_color());

        self.draw_rect(
            canvas,
            0.0,
            0.0,
            layout.final_width,
            layout.max_height + cfg.time_padding as f64 * 2.0,
            "background",
            RGBA::WHITE,
        );
        self.draw_rect(
            canvas,
            0.0,
            layout.max_height + cfg.time_padding as f64 * 2.0,
            layout.final_width,
            cfg.meta_size as f64 + cfg.time_padding as f64 * 2.0,
            "meta",
            RGBA::WHITE,
        );
        self.draw_line(
            canvas,
            0.0,
            layout.max_height + cfg.time_padding as f64 * 2.0,
            layout.final_width,
            layout.max_height + cfg.time_padding as f64 * 2.0,
            "meta-line",
            RGBA::rgb(0xe2, 0xe2, 0xe2),
            2.0,
        );

        self.draw_jacket(
            canvas,
            score,
            cfg.lane_padding as f64 * 2.0,
            layout.max_height + cfg.time_padding as f64 * 3.0,
            cfg.meta_size as f64,
            cfg.meta_size as f64,
        )?;

        let title = [score.meta.title.as_deref(), score.meta.artist.as_deref()]
            .iter()
            .filter_map(|x| *x)
            .collect::<Vec<_>>()
            .join(" - ");
        let title = if title.is_empty() {
            "Untitled".to_string()
        } else {
            title
        };
        self.draw_text(
            canvas,
            &title,
            (cfg.meta_size + cfg.lane_padding * 4) as f64,
            cfg.meta_size as f64 + layout.max_height + cfg.time_padding as f64 * 3.0 - 16.0,
            "title",
            TextDefaults::new(RGBA::BLACK, 96.0, 900),
            TextAnchor::Start,
        );

        let subtitle_parts: Vec<String> = [
            score
                .meta
                .difficulty
                .as_ref()
                .filter(|d| !d.is_empty() && d.parse::<f64>() != Ok(0.0))
                .map(|d| d.to_uppercase()),
            score
                .meta
                .playlevel
                .as_ref()
                .filter(|p| !p.is_empty())
                .cloned(),
            Some(format!(
                "Code by pjsekai.moe, Modified by bilibili @xfl03 (3-3.dev),Generated by {}",
                self.drawing.config.generator
            )),
        ]
        .iter()
        .filter_map(|x| x.clone())
        .collect();
        let subtitle = subtitle_parts.join(" ");
        self.draw_text(
            canvas,
            &subtitle,
            (cfg.meta_size + cfg.lane_padding * 4) as f64,
            cfg.meta_size as f64 / 3.0 + layout.max_height + cfg.time_padding as f64 * 3.0 - 8.0,
            "subtitle",
            TextDefaults::new(RGBA::BLACK, 48.0, 700),
            TextAnchor::Start,
        );

        let notes_snapshot = score.notes.clone();
        let render_index = RenderIndex::new(&score.active_notes, &notes_snapshot);
        let mut x_offset = 0.0;
        if should_render_segments_parallel(layout) {
            let segment_rasters = self.render_segment_rasters(
                &*score,
                lyric,
                layout,
                &notes_snapshot,
                &render_index,
            )?;
            for (segment, raster) in layout.segments.iter().zip(&segment_rasters) {
                let y_offset = layout.max_height - segment.height + cfg.time_padding as f64;
                draw_segment_image(
                    canvas,
                    raster,
                    x_offset + cfg.lane_padding as f64,
                    y_offset,
                    segment.width,
                    segment.height,
                )?;
                x_offset += segment.width;
            }
        } else {
            for segment in &layout.segments {
                let y_offset = layout.max_height - segment.height + cfg.time_padding as f64;
                canvas.save();
                canvas.clip_rect(
                    Rect::from_xywh(
                        as_f32(x_offset + cfg.lane_padding as f64),
                        as_f32(y_offset),
                        as_f32(segment.width),
                        as_f32(segment.height),
                    ),
                    None,
                    Some(true),
                );
                canvas.translate((as_f32(x_offset + cfg.lane_padding as f64), as_f32(y_offset)));
                self.draw_segment(
                    canvas,
                    score,
                    lyric,
                    segment,
                    &notes_snapshot,
                    &render_index,
                );
                canvas.restore();
                x_offset += segment.width;
            }
        }

        debug_assert!((x_offset - layout.total_width).abs() < 0.001);
        Ok(())
    }

    fn render_segment_rasters(
        &self,
        score: &Score,
        lyric: Option<&Lyric>,
        layout: &Layout,
        notes_snapshot: &[NoteData],
        render_index: &RenderIndex,
    ) -> Result<Vec<SegmentRaster>, SkiaDirectError> {
        if layout.segments.is_empty() {
            return Ok(Vec::new());
        }

        std::thread::scope(|scope| {
            let drawing = self.drawing;
            let worker_count = std::thread::available_parallelism()
                .map(|threads| threads.get())
                .unwrap_or(1)
                .min(layout.segments.len());
            let chunk_size = layout.segments.len().div_ceil(worker_count);
            let handles: Vec<_> = layout
                .segments
                .chunks(chunk_size)
                .enumerate()
                .map(|(chunk_idx, segments)| {
                    let start_idx = chunk_idx * chunk_size;
                    let styles = self.styles.clone();
                    let note_assets = self.note_assets.clone();
                    let custom_typefaces = self.custom_typefaces.clone();
                    scope.spawn(move || {
                        let renderer = DirectRenderer {
                            drawing,
                            styles,
                            font_mgr: FontMgr::default(),
                            custom_typefaces,
                            note_assets,
                            font_cache: Mutex::new(HashMap::new()),
                        };
                        let mut rasters = Vec::with_capacity(segments.len());
                        for (offset, segment) in segments.iter().enumerate() {
                            rasters.push((
                                start_idx + offset,
                                renderer.render_segment_raster(
                                    score,
                                    lyric,
                                    segment,
                                    notes_snapshot,
                                    render_index,
                                )?,
                            ));
                        }
                        Ok::<_, SkiaDirectError>(rasters)
                    })
                })
                .collect();
            let mut rasters = Vec::new();
            rasters.resize_with(layout.segments.len(), || None);
            for handle in handles {
                for (idx, raster) in handle.join().map_err(|_| SkiaDirectError::RenderWorker)?? {
                    rasters[idx] = Some(raster);
                }
            }
            rasters
                .into_iter()
                .map(|raster| raster.ok_or(SkiaDirectError::Surface))
                .collect()
        })
    }

    fn render_segment_raster(
        &self,
        score: &Score,
        lyric: Option<&Lyric>,
        segment: &Segment,
        notes_snapshot: &[NoteData],
        render_index: &RenderIndex,
    ) -> Result<SegmentRaster, SkiaDirectError> {
        let width = round_even(segment.width) as i32;
        let height = round_even(segment.height) as i32;
        if width <= 0 || height <= 0 {
            return Err(SkiaDirectError::InvalidSize);
        }

        let mut segment_score = segment_score(score);
        let mut surface =
            surfaces::raster_n32_premul((width, height)).ok_or(SkiaDirectError::Surface)?;
        self.draw_segment(
            surface.canvas(),
            &mut segment_score,
            lyric,
            segment,
            notes_snapshot,
            render_index,
        );

        Ok(SegmentRaster {
            width,
            height,
            image: surface.image_snapshot(),
        })
    }

    fn draw_segment(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        lyric: Option<&Lyric>,
        segment: &Segment,
        notes_snapshot: &[NoteData],
        render_index: &RenderIndex,
    ) {
        let cfg = &self.drawing.config;
        let bar_start_f = Fraction::from_integer(segment.start as i64);
        let bar_stop_f = Fraction::from_integer(segment.stop as i64);
        let chart_height = segment.height - cfg.time_padding as f64 * 2.0;

        self.draw_rect(
            canvas,
            0.0,
            0.0,
            segment.width,
            segment.height,
            "background",
            RGBA::WHITE,
        );
        self.draw_rect(
            canvas,
            cfg.lane_padding as f64,
            0.0,
            cfg.lane_width as f64 * cfg.n_lanes as f64,
            segment.height,
            "lane",
            RGBA::rgb(0xcf, 0xd8, 0xdc),
        );

        for cover in &self.drawing.special_cover_objects {
            match cover {
                CoverObject::Text {
                    bar_from,
                    css_class,
                    text,
                } => {
                    if *bar_from < bar_start_f - Fraction::from_f64(0.2)
                        || *bar_from >= bar_stop_f - Fraction::from_f64(0.1)
                    {
                        continue;
                    }
                    let y = cfg.time_height * score.get_time_delta_f64(*bar_from, bar_stop_f)
                        + cfg.time_padding as f64;
                    let x = cfg.lane_width as f64 * cfg.n_lanes as f64
                        + cfg.lane_padding as f64 * 2.0
                        - 3.0;
                    self.draw_rotated_text(
                        canvas,
                        text,
                        x,
                        y,
                        x,
                        y,
                        css_class,
                        TextDefaults::new(RGBA::WHITE, 38.0, 400),
                        TextAnchor::Start,
                    );
                }
                CoverObject::Rect {
                    bar_from,
                    css_class,
                    bar_to,
                } => {
                    let cover_from = if *bar_from > bar_start_f - Fraction::from_f64(0.2) {
                        *bar_from
                    } else {
                        bar_start_f - Fraction::from_f64(0.2)
                    };
                    let cover_to = if *bar_to < bar_stop_f + Fraction::from_f64(0.2) {
                        *bar_to
                    } else {
                        bar_stop_f + Fraction::from_f64(0.2)
                    };
                    if cover_to <= cover_from {
                        continue;
                    }
                    let y = cfg.time_height * score.get_time_delta_f64(cover_to, bar_stop_f)
                        + cfg.time_padding as f64;
                    let h = cfg.time_height * score.get_time_delta_f64(cover_from, cover_to);
                    self.draw_rect(
                        canvas,
                        cfg.lane_padding as f64,
                        y,
                        cfg.lane_width as f64 * cfg.n_lanes as f64,
                        h,
                        css_class,
                        RGBA::rgb(0x78, 0x90, 0x9c),
                    );
                }
            }
        }

        for lane in (0..=cfg.n_lanes).step_by(2) {
            let x = cfg.lane_width as f64 * lane as f64 + cfg.lane_padding as f64;
            self.draw_line(
                canvas,
                x,
                0.0,
                x,
                segment.height,
                "lane-line",
                RGBA::rgb(0xe2, 0xe2, 0xe2),
                1.0,
            );
        }

        for bar in segment.start..=segment.stop {
            let bar_f = Fraction::from_integer(bar as i64);
            let y = cfg.time_height * score.get_time_delta_f64(bar_f, bar_stop_f)
                + cfg.time_padding as f64;
            let x1 = cfg.lane_padding as f64;
            let x2 = cfg.lane_width as f64 * cfg.n_lanes as f64 + cfg.lane_padding as f64;
            self.draw_line(
                canvas,
                x1,
                y,
                x2,
                y,
                "bar-line",
                RGBA::rgb(0xe2, 0xe2, 0xe2),
                4.0,
            );

            let event = score.get_event(bar_f);
            let bar_length = event
                .bar_length
                .unwrap_or(Fraction::from_integer(4))
                .to_f64()
                .ceil() as i32;
            let bar_length_frac = event.bar_length.unwrap_or(Fraction::from_integer(4));
            for beat_i in 1..bar_length {
                let beat_bar = bar_f + Fraction::new(beat_i as i64, 1) / bar_length_frac;
                let beat_y = cfg.time_height * score.get_time_delta_f64(beat_bar, bar_stop_f)
                    + cfg.time_padding as f64;
                self.draw_line(
                    canvas,
                    x1,
                    beat_y,
                    x2,
                    beat_y,
                    "beat-line",
                    RGBA::rgb(0xe2, 0xe2, 0xe2),
                    1.0,
                );
            }
        }

        let speed_lines = self.draw_event_labels(canvas, score, segment.start, segment.stop);
        if let Some(lyric) = lyric {
            self.draw_lyrics(canvas, score, lyric, bar_start_f, bar_stop_f);
        }

        let layers =
            self.collect_note_layers(render_index, notes_snapshot, bar_start_f, bar_stop_f);
        let mut amongs = Vec::new();
        for &idx in &layers.slide_paths {
            self.draw_slide_path(canvas, score, notes_snapshot, idx, bar_stop_f, &mut amongs);
        }
        for &idx in &layers.notes {
            self.draw_note(canvas, score, notes_snapshot, idx, bar_stop_f);
        }
        for among in amongs {
            self.draw_among(
                canvas,
                among.kind,
                among.x,
                among.y,
                among.width,
                among.height,
            );
        }
        for &idx in layers.flicks.iter().rev() {
            self.draw_flick(canvas, score, notes_snapshot, idx, bar_stop_f);
        }
        for tick in layers.ticks {
            self.draw_tick(canvas, score, notes_snapshot, tick, bar_stop_f);
        }
        for speed in speed_lines {
            let x1 = cfg.lane_padding as f64;
            let x2 = cfg.lane_width as f64 * cfg.n_lanes as f64 + cfg.lane_padding as f64;
            self.draw_line(
                canvas,
                x1,
                speed.y,
                x2,
                speed.y,
                "speed-line",
                RGBA::rgb(0xff, 0x33, 0xff),
                1.0,
            );
            self.draw_text(
                canvas,
                &format!("{}x", format_g(speed.speed)),
                x2 - 2.0,
                speed.y - 2.0,
                "speed-text",
                TextDefaults::new(RGBA::rgb(0xff, 0x33, 0xff), 12.0, 400),
                TextAnchor::End,
            );
        }

        let _ = chart_height;
    }

    fn draw_event_labels(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        bar_start: i32,
        bar_stop: i32,
    ) -> Vec<SpeedLine> {
        let cfg = &self.drawing.config;
        let bar_start_f = Fraction::from_integer(bar_start as i64);
        let bar_stop_f = Fraction::from_integer(bar_stop as i64);
        let visible_from = bar_start_f - Fraction::from_integer(1);
        let visible_to = bar_stop_f + Fraction::from_integer(1);
        let mut speed_lines = Vec::new();
        let mut print_events: Vec<Event> = Vec::new();
        let mut all_events: Vec<Event> = (bar_start..=bar_stop)
            .map(|i| Event::new(Fraction::from_integer(i as i64)))
            .collect();
        all_events.extend(
            score
                .events
                .iter()
                .filter(|event| visible_from <= event.bar && event.bar < visible_to)
                .cloned(),
        );
        all_events.sort_by(|a, b| {
            a.bar
                .partial_cmp(&b.bar)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for event in &all_events {
            if let Some(speed) = event.speed {
                let y = cfg.time_height * score.get_time_delta_f64(event.bar, bar_stop_f)
                    + cfg.time_padding as f64;
                speed_lines.push(SpeedLine { y, speed });
                continue;
            }

            if let Some(last) = print_events.last_mut() {
                if (event.bar - last.bar).to_f64() <= 1.0 / 16.0 {
                    last.merge_from(event);
                } else {
                    print_events.push(event.clone());
                }
            } else {
                print_events.push(event.clone());
            }

            let special = event.bpm.is_some()
                || event.bar_length.is_some()
                || event.speed.is_some()
                || event.section.is_some()
                || event.text.is_some();
            let y = cfg.time_height * score.get_time_delta_f64(event.bar, bar_stop_f)
                + cfg.time_padding as f64;
            self.draw_line(
                canvas,
                0.0,
                y,
                cfg.lane_padding as f64,
                y,
                if special {
                    "event-flag"
                } else {
                    "bar-count-flag"
                },
                if special {
                    RGBA::rgb(0xfe, 0xe3, 0x00)
                } else {
                    RGBA::WHITE
                },
                4.0,
            );
        }

        for event in &print_events {
            let mut parts = Vec::new();
            if event.bar.trunc() == *event.bar.numer() && *event.bar.denom() == 1 {
                parts.push(format!("#{}", format_g(event.bar.to_f64())));
            }
            if let Some(bpm) = event.bpm {
                parts.push(format!("{} BPM", format_g(bpm.to_f64())));
            }
            if let Some(bl) = event.bar_length {
                parts.push(format!("{}/4", format_g(bl.to_f64())));
            }
            if let Some(ref section) = event.section {
                parts.push(section.clone());
            }
            if let Some(ref text) = event.text {
                parts.push(text.clone());
            }

            let text = parts.join(", ");
            if text.is_empty() {
                continue;
            }
            let special = event.bpm.is_some()
                || event.bar_length.is_some()
                || event.speed.is_some()
                || event.section.is_some()
                || event.text.is_some();
            let y = cfg.time_height * score.get_time_delta_f64(event.bar, bar_stop_f)
                + cfg.time_padding as f64;
            self.draw_rotated_text(
                canvas,
                &text,
                cfg.lane_padding as f64 + 8.0,
                y - cfg.lane_width as f64 * 1.5,
                cfg.lane_padding as f64,
                y,
                if special {
                    "event-text"
                } else {
                    "bar-count-text"
                },
                TextDefaults::new(
                    if special {
                        RGBA::rgb(0xfe, 0xe3, 0x00)
                    } else {
                        RGBA::WHITE
                    },
                    12.0,
                    900,
                ),
                TextAnchor::Start,
            );
        }

        speed_lines
    }

    fn draw_lyrics(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        lyric: &Lyric,
        bar_start_f: Fraction,
        bar_stop_f: Fraction,
    ) {
        let cfg = &self.drawing.config;
        for word in &lyric.words {
            if !(bar_start_f - Fraction::from_integer(1) <= word.bar
                && word.bar < bar_stop_f + Fraction::from_integer(1))
            {
                continue;
            }
            let y = cfg.time_height * score.get_time_delta_f64(word.bar, bar_stop_f)
                + cfg.time_padding as f64;
            let x = cfg.lane_width as f64 * cfg.n_lanes as f64 + cfg.lane_padding as f64;
            self.draw_rotated_text(
                canvas,
                &word.text,
                x,
                y + 16.0,
                x,
                y,
                "lyric-text",
                TextDefaults::new(RGBA::WHITE, 12.0, 400),
                TextAnchor::Start,
            );
        }
    }

    fn collect_note_layers(
        &self,
        render_index: &RenderIndex,
        notes_snapshot: &[NoteData],
        bar_start_f: Fraction,
        bar_stop_f: Fraction,
    ) -> SegmentLayers {
        let mut layers = SegmentLayers::default();
        let window_start = bar_start_f - Fraction::from_integer(1);
        let window_stop = bar_stop_f + Fraction::from_integer(1);
        let start = render_index
            .note_bars
            .partition_point(|&bar| bar < window_start);
        let stop = render_index
            .note_bars
            .partition_point(|&bar| bar < window_stop);

        for &note_idx in &render_index.active_notes[start..stop] {
            let note = &notes_snapshot[note_idx];
            if note.is_slide() && !note_visible(note, notes_snapshot, bar_start_f, bar_stop_f) {
                continue;
            }
            self.collect_visible_note(&mut layers, render_index, notes_snapshot, note_idx, note);
        }

        let slide_stop = render_index
            .slide_start_bars
            .partition_point(|&bar| bar < window_start);
        for &note_idx in &render_index.slide_starts[..slide_stop] {
            let note = &notes_snapshot[note_idx];
            if note_visible(note, notes_snapshot, bar_start_f, bar_stop_f) {
                self.collect_visible_note(
                    &mut layers,
                    render_index,
                    notes_snapshot,
                    note_idx,
                    note,
                );
            }
        }

        layers
    }

    fn collect_visible_note(
        &self,
        layers: &mut SegmentLayers,
        render_index: &RenderIndex,
        notes_snapshot: &[NoteData],
        note_idx: NoteIdx,
        note: &NoteData,
    ) {
        if let Some(tick_val) = note.is_tick(notes_snapshot) {
            if tick_val {
                let next_idx = render_index.next_ticks[note_idx];
                layers.ticks.push(TickCommand::Text { note_idx, next_idx });
            } else {
                layers.ticks.push(TickCommand::Short { bar: note.bar() });
            }
        }

        match note {
            NoteData::Tap(..) => layers.notes.push(note_idx),
            NoteData::Directional(..) => {
                layers.flicks.push(note_idx);
                layers.notes.push(note_idx);
            }
            NoteData::Slide(_, slide) => {
                if !slide.decoration {
                    match SlideType::from_i32(note.note_type()) {
                        Some(SlideType::Start) => {
                            layers.slide_paths.push(note_idx);
                            layers.notes.push(note_idx);
                        }
                        Some(SlideType::End) => {
                            if slide.directional_idx != NO_NOTE {
                                layers.flicks.push(note_idx);
                            }
                            layers.notes.push(note_idx);
                        }
                        _ => {}
                    }
                } else {
                    if matches!(
                        SlideType::from_i32(note.note_type()),
                        Some(SlideType::Start)
                    ) {
                        layers.slide_paths.push(note_idx);
                    }
                    if slide.tap_idx != NO_NOTE {
                        layers.notes.push(slide.tap_idx);
                        if slide.directional_idx != NO_NOTE {
                            layers.flicks.push(note_idx);
                        }
                    }
                }
            }
        }
    }

    fn draw_slide_path(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        arena: &[NoteData],
        start_idx: NoteIdx,
        bar_stop: Fraction,
        amongs: &mut Vec<AmongCommand>,
    ) {
        let cfg = &self.drawing.config;
        let mut lefts: Vec<BezierPoints> = Vec::new();
        let mut rights: Vec<BezierPoints> = Vec::new();
        let mut cur_idx = start_idx;

        loop {
            let cur_type = arena[cur_idx].note_type();
            if matches!(SlideType::from_i32(cur_type), Some(SlideType::End)) {
                break;
            }

            let Some(slide) = arena[cur_idx].as_slide() else {
                break;
            };
            if slide.next_idx == NO_NOTE {
                break;
            }

            let mut relay_amongs = Vec::new();
            let mut next_idx = slide.next_idx;
            loop {
                let next_type = arena[next_idx].note_type();
                if matches!(SlideType::from_i32(next_type), Some(SlideType::Relay)) {
                    relay_amongs.push(next_idx);
                }

                let Some(next_slide) = arena[next_idx].as_slide() else {
                    break;
                };
                if next_slide.is_path(next_type) {
                    break;
                }
                if next_slide.next_idx == NO_NOTE {
                    break;
                }
                next_idx = next_slide.next_idx;
            }

            let (left, right) = bezier_coordinates(
                &self.drawing.config,
                score,
                arena,
                cur_idx,
                next_idx,
                bar_stop,
            );
            for &among_idx in &relay_amongs {
                let among_bar = arena[among_idx].bar();
                let y = cfg.time_height * score.get_time_delta_f64(among_bar, bar_stop)
                    + cfg.time_padding as f64;
                let x_l = binary_solution_for_x(y, &left);
                let x_r = binary_solution_for_x(y, &right);
                let x = (x_l + x_r) / 2.0;
                let w = cfg.lane_width as f64;
                let h = cfg.lane_width as f64;
                amongs.push(AmongCommand {
                    kind: if arena[among_idx].is_critical(arena) {
                        AmongKind::LongAmongCritical
                    } else {
                        AmongKind::LongAmong
                    },
                    x: x - w / 2.0,
                    y: y - h / 2.0,
                    width: w,
                    height: h,
                });
            }

            lefts.push(left);
            rights.push(right);
            cur_idx = next_idx;
        }

        if lefts.is_empty() {
            return;
        }

        let is_critical = arena[start_idx].is_critical(arena);
        let is_decoration = arena[start_idx]
            .as_slide()
            .map(|s| s.decoration)
            .unwrap_or(false);
        let (class_name, fallback) = if is_decoration {
            if is_critical {
                ("decoration-critical", RGBA::rgba(0xfc, 0xf1, 0xc3, 0x99))
            } else {
                ("decoration", RGBA::rgba(0xc9, 0xfc, 0xe2, 0x99))
            }
        } else if is_critical {
            ("slide-critical", RGBA::rgba(0xfc, 0xf1, 0xc3, 0xcc))
        } else {
            ("slide", RGBA::rgba(0xc9, 0xfc, 0xe2, 0xcc))
        };

        let mut path = PathBuilder::new();
        for (i, left) in lefts.iter().enumerate() {
            if i == 0 {
                path.move_to(point(left[0]));
            }
            path.cubic_to(point(left[1]), point(left[2]), point(left[3]));
        }
        for (i, right) in rights.iter().rev().enumerate() {
            if i == 0 {
                path.line_to(point(right[3]));
            }
            path.cubic_to(point(right[2]), point(right[1]), point(right[0]));
        }
        path.close();
        let path = path.detach();
        let paint = fill_paint(self.color(class_name, fallback));
        canvas.draw_path(&path, &paint);
    }

    fn draw_note(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        bar_stop: Fraction,
    ) {
        let cfg = &self.drawing.config;
        let note = &arena[note_idx];
        if note.is_none(arena) {
            return;
        }

        let y = cfg.time_height * score.get_time_delta_f64(note.bar(), bar_stop)
            + cfg.time_padding as f64;
        let x = cfg.lane_width as f64 * (note.lane() as f64 - 2.5) + cfg.lane_padding as f64;
        let w = cfg.lane_width as f64 * (note.width() + 1) as f64;
        let h = cfg.lane_width as f64 / 64.0 * 56.0 * 2.0;

        let note_number = if note.is_trend(arena) {
            self.draw_friction_among(canvas, score, arena, note_idx, bar_stop);
            if note.is_critical(arena) {
                5
            } else if note.is_directional() {
                6
            } else {
                4
            }
        } else if note.is_critical(arena) {
            0
        } else if note.is_directional() {
            3
        } else if note.is_slide() {
            if matches!(SlideType::from_i32(note.note_type()), Some(SlideType::End)) {
                if let Some(slide) = note.as_slide() {
                    if slide.directional_idx != NO_NOTE {
                        3
                    } else {
                        1
                    }
                } else {
                    1
                }
            } else {
                1
            }
        } else {
            2
        };

        if self.draw_note_image_asset(canvas, note_number, note.width() + 1, x, y - h / 2.0, w, h) {
            return;
        }

        let (x, w) = if matches!(note_number, 4..=6) {
            (
                cfg.lane_width as f64 * (note.lane() as f64 - 2.125) + cfg.lane_padding as f64,
                cfg.lane_width as f64 * (note.width() as f64 + 0.25),
            )
        } else {
            (x, w)
        };
        draw_vector_note_body(canvas, note_number, x, y - h / 2.0, w, h);
    }

    fn draw_friction_among(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        bar_stop: Fraction,
    ) {
        let cfg = &self.drawing.config;
        let note = &arena[note_idx];
        let y = cfg.time_height * score.get_time_delta_f64(note.bar(), bar_stop)
            + cfg.time_padding as f64;
        let x = cfg.lane_width as f64 * (note.lane() as f64 + note.width() as f64 / 2.0 - 2.0)
            + cfg.lane_padding as f64;
        let w = cfg.lane_width as f64 * 0.75;
        let h = cfg.lane_width as f64 * 0.75;
        let kind = if note.is_critical(arena) {
            AmongKind::FrictionCritical
        } else if note.is_directional() {
            AmongKind::FrictionFlick
        } else {
            AmongKind::FrictionLong
        };
        self.draw_among(canvas, kind, x - w / 2.0, y - h / 2.0, w, h);
    }

    fn draw_flick(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        bar_stop: Fraction,
    ) {
        let cfg = &self.drawing.config;
        let note = &arena[note_idx];
        if note.is_none(arena) {
            return;
        }

        let y = cfg.time_height * score.get_time_delta_f64(note.bar(), bar_stop)
            + cfg.time_padding as f64;
        let dir_type = if note.is_directional() {
            DirectionalType::from_i32(note.note_type())
        } else if note.is_slide() {
            note.as_slide().and_then(|slide| {
                if slide.directional_idx != NO_NOTE {
                    DirectionalType::from_i32(arena[slide.directional_idx].note_type())
                } else {
                    None
                }
            })
        } else {
            None
        };

        let flick_type = match dir_type {
            Some(DirectionalType::UpperLeft) => Some(DirectionalType::UpperLeft),
            Some(DirectionalType::UpperRight) => Some(DirectionalType::UpperRight),
            Some(DirectionalType::Up) => Some(DirectionalType::Up),
            Some(_) => None,
            None => Some(DirectionalType::Up),
        };
        let Some(flick_type) = flick_type else {
            return;
        };

        let width = if note.width() < 6 { note.width() } else { 6 };
        let h0 = cfg.flick_height as f64;
        let h = h0 * ((width as f64 + 3.0) / 3.0_f64).powf(0.75);
        let w = h0 * 1.5 * ((width as f64 + 0.5) / 3.0_f64).powf(0.75);
        let x = cfg.lane_width as f64 * (note.lane() as f64 - 2.0 + note.width() as f64 / 2.0)
            + cfg.lane_padding as f64;
        let bias = match flick_type {
            DirectionalType::UpperLeft => -(cfg.note_size as f64) / 4.0,
            DirectionalType::UpperRight => cfg.note_size as f64 / 4.0,
            _ => 0.0,
        };
        let is_diagonal = matches!(
            flick_type,
            DirectionalType::UpperLeft | DirectionalType::UpperRight
        );
        let is_crit = note.is_critical(arena);
        let img_x = x - w / 2.0 + bias;
        let img_y = y + cfg.note_size as f64 / 4.0 - h;
        if self.draw_flick_image_asset(
            canvas,
            is_crit,
            is_diagonal,
            matches!(flick_type, DirectionalType::UpperRight),
            width,
            img_x,
            img_y,
            w,
            h,
            x + bias,
        ) {
            return;
        }

        draw_vector_flick(
            canvas,
            is_crit,
            is_diagonal,
            matches!(flick_type, DirectionalType::UpperRight),
            img_x,
            img_y,
            w,
            h,
        );
    }

    fn draw_tick(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        arena: &[NoteData],
        tick: TickCommand,
        bar_stop: Fraction,
    ) {
        let cfg = &self.drawing.config;
        match tick {
            TickCommand::Short { bar } => {
                let y = cfg.time_height * score.get_time_delta_f64(bar, bar_stop)
                    + cfg.time_padding as f64;
                self.draw_line(
                    canvas,
                    cfg.lane_padding as f64 - cfg.tick_2_length as f64,
                    y,
                    cfg.lane_padding as f64,
                    y,
                    "tick-line",
                    RGBA::rgb(0xe2, 0xe2, 0xe2),
                    1.0,
                );
            }
            TickCommand::Text { note_idx, next_idx } => {
                let note = &arena[note_idx];
                let y = cfg.time_height * score.get_time_delta_f64(note.bar(), bar_stop)
                    + cfg.time_padding as f64;
                let next = &arena[next_idx];
                let interval_frac =
                    if next.bar() == note.bar() || (next.bar() - note.bar()).to_f64() > 1.0 {
                        Fraction::from_integer(note.bar().floor() + 1) - note.bar()
                    } else if (next.bar() - note.bar()).to_f64() > 0.5
                        && next.bar().floor() != note.bar().floor()
                    {
                        Fraction::from_integer(note.bar().floor() + 1) - note.bar()
                    } else {
                        next.bar() - note.bar()
                    };
                self.draw_tick_with_interval(canvas, score, y, interval_frac, note.bar());
            }
        }
    }

    fn draw_tick_with_interval(
        &self,
        canvas: &skia_safe::Canvas,
        score: &mut Score,
        y: f64,
        interval: Fraction,
        bar: Fraction,
    ) {
        let cfg = &self.drawing.config;
        let event = score.get_event(bar);
        let bar_length = event.bar_length.unwrap_or(Fraction::from_integer(4));
        let interval = (interval * bar_length / Fraction::from_integer(4)).limit_denominator(100);
        if interval == Fraction::zero() {
            return;
        }

        let text = if *interval.numer() != 1 {
            format!("{}/{}", interval.numer(), interval.denom())
        } else {
            format!("/{}", interval.denom())
        };
        self.draw_line(
            canvas,
            cfg.lane_padding as f64 - cfg.tick_length as f64,
            y,
            cfg.lane_padding as f64,
            y,
            "tick-line",
            RGBA::rgb(0xe2, 0xe2, 0xe2),
            1.0,
        );
        self.draw_text(
            canvas,
            &text,
            cfg.lane_padding as f64 - 4.0,
            y - 2.0,
            "tick-text",
            TextDefaults::new(RGBA::rgb(0xe2, 0xe2, 0xe2), 12.0, 400),
            TextAnchor::End,
        );
    }

    fn draw_among(
        &self,
        canvas: &skia_safe::Canvas,
        kind: AmongKind,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) {
        if self.draw_among_image_asset(canvas, kind, x, y, width, height) {
            return;
        }
        draw_vector_among(canvas, kind, x, y, width, height);
    }

    fn draw_note_image_asset(
        &self,
        canvas: &skia_safe::Canvas,
        note_number: i32,
        width_units: i32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> bool {
        if let Some(image) = self.note_assets.sliced_note(note_number, width_units) {
            draw_image(canvas, image, x, y, width, height);
            return true;
        }

        let name = format!("notes_{note_number}");
        let Some(image) = self.note_assets.get(&name) else {
            return false;
        };
        draw_sliced_note_image(canvas, image, x, y, width, height);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_flick_image_asset(
        &self,
        canvas: &skia_safe::Canvas,
        is_critical: bool,
        is_diagonal: bool,
        flip_right: bool,
        width_lanes: i32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        origin_x: f64,
    ) -> bool {
        let name = format!(
            "notes_flick_arrow{}_{:02}{}",
            if is_critical { "_crtcl" } else { "" },
            width_lanes,
            if is_diagonal { "_diagonal" } else { "" },
        );
        let Some(image) = self.note_assets.get(&name) else {
            return false;
        };
        if flip_right {
            canvas.save();
            canvas.translate((as_f32(origin_x), 0.0));
            canvas.scale((-1.0, 1.0));
            canvas.translate((as_f32(-origin_x), 0.0));
            draw_image(canvas, image, x, y, width, height);
            canvas.restore();
        } else {
            draw_image(canvas, image, x, y, width, height);
        }
        true
    }

    fn draw_among_image_asset(
        &self,
        canvas: &skia_safe::Canvas,
        kind: AmongKind,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> bool {
        let name = match kind {
            AmongKind::LongAmong => "notes_long_among",
            AmongKind::LongAmongCritical => "notes_long_among_crtcl",
            AmongKind::FrictionLong => "notes_friction_among_long",
            AmongKind::FrictionFlick => "notes_friction_among_flick",
            AmongKind::FrictionCritical => "notes_friction_among_crtcl",
        };
        let Some(image) = self.note_assets.get(name) else {
            return false;
        };
        draw_image(canvas, image, x, y, width, height);
        true
    }

    fn draw_jacket(
        &self,
        canvas: &skia_safe::Canvas,
        score: &Score,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), SkiaDirectError> {
        let rect = Rect::from_xywh(as_f32(x), as_f32(y), as_f32(width), as_f32(height));
        let jacket = score.meta.jacket.as_deref().filter(|s| !s.is_empty());
        let Some(path) = jacket.and_then(local_image_path) else {
            self.draw_jacket_placeholder(canvas, x, y, width, height);
            return Ok(());
        };

        let bytes = fs::read(&path).map_err(|source| SkiaDirectError::Io {
            path: path.clone(),
            source,
        })?;
        let image = Image::from_encoded(Data::new_copy(&bytes))
            .ok_or_else(|| SkiaDirectError::Decode(path.clone()))?;
        let paint = fill_paint(RGBA::WHITE);
        let sampling = SamplingOptions::from(FilterMode::Linear);
        canvas.draw_image_rect_with_sampling_options(&image, None, &rect, sampling, &paint);
        Ok(())
    }

    fn draw_jacket_placeholder(
        &self,
        canvas: &skia_safe::Canvas,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) {
        let fill = self.color("lane", RGBA::rgb(0xcf, 0xd8, 0xdc));
        canvas.draw_rect(&rect(x, y, width, height), &fill_paint(fill));
        self.draw_line(
            canvas,
            x,
            y,
            x + width,
            y + height,
            "meta-line",
            RGBA::rgb(0xe2, 0xe2, 0xe2),
            2.0,
        );
        self.draw_line(
            canvas,
            x + width,
            y,
            x,
            y + height,
            "meta-line",
            RGBA::rgb(0xe2, 0xe2, 0xe2),
            2.0,
        );
    }

    fn draw_rect(
        &self,
        canvas: &skia_safe::Canvas,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        class_name: &str,
        fallback: RGBA,
    ) {
        let paint = fill_paint(self.color(class_name, fallback));
        canvas.draw_rect(&rect(x, y, width, height), &paint);
    }

    fn draw_line(
        &self,
        canvas: &skia_safe::Canvas,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        class_name: &str,
        fallback: RGBA,
        fallback_width: f32,
    ) {
        let style = self.styles.get(class_name);
        let color = style.stroke.unwrap_or(fallback);
        let width = style.stroke_width.unwrap_or(fallback_width);
        let paint = stroke_paint(color, width);
        canvas.draw_line((as_f32(x1), as_f32(y1)), (as_f32(x2), as_f32(y2)), &paint);
    }

    fn draw_text(
        &self,
        canvas: &skia_safe::Canvas,
        text: &str,
        x: f64,
        y: f64,
        class_name: &str,
        defaults: TextDefaults,
        anchor: TextAnchor,
    ) {
        if text.is_empty() {
            return;
        }
        let style = self.styles.get(class_name);
        let color = style.fill.unwrap_or(defaults.color);
        let size = style.font_size.unwrap_or(defaults.size);
        let weight = style.font_weight.unwrap_or(defaults.weight);
        let paint = fill_paint(color);
        let font = self.font(size, weight, style.font_families.as_deref(), text);
        let mut draw_x = x;
        if matches!(anchor, TextAnchor::End) {
            let (width, _) = font.measure_str(text, Some(&paint));
            draw_x -= width as f64;
        }
        canvas.draw_str(text, (as_f32(draw_x), as_f32(y)), &font, &paint);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_rotated_text(
        &self,
        canvas: &skia_safe::Canvas,
        text: &str,
        x: f64,
        y: f64,
        pivot_x: f64,
        pivot_y: f64,
        class_name: &str,
        defaults: TextDefaults,
        anchor: TextAnchor,
    ) {
        canvas.save();
        canvas.translate((as_f32(pivot_x), as_f32(pivot_y)));
        canvas.rotate(-90.0, None);
        self.draw_text(
            canvas,
            text,
            x - pivot_x,
            y - pivot_y,
            class_name,
            defaults,
            anchor,
        );
        canvas.restore();
    }

    fn color(&self, class_name: &str, fallback: RGBA) -> RGBA {
        self.styles.get(class_name).fill.unwrap_or(fallback)
    }

    fn font(&self, size: f32, weight: i32, font_families: Option<&[String]>, text: &str) -> Font {
        let required_cjk_glyphs = required_cjk_glyphs(text);
        let key = FontKey::new(size, weight, font_families, &required_cjk_glyphs);
        let mut cache = self.font_cache.lock().expect("font cache lock poisoned");
        if let Some(font) = cache.get(&key) {
            return font.clone();
        }

        let bold = weight >= 700;
        let style = if bold {
            FontStyle::bold()
        } else {
            FontStyle::normal()
        };
        let typeface = font_families
            .into_iter()
            .flatten()
            .map(String::as_str)
            .chain(FALLBACK_FONT_FAMILIES.iter().copied())
            .filter_map(|family| self.match_font_family(family, style, &required_cjk_glyphs))
            .next()
            .or_else(|| {
                if required_cjk_glyphs.is_empty() {
                    self.font_mgr.legacy_make_typeface(None, style)
                } else {
                    self.match_any_custom_font(style, &required_cjk_glyphs)
                }
            });
        let mut font = if let Some(typeface) = typeface {
            Font::new(typeface, Some(size))
        } else {
            let mut font = Font::default();
            font.set_size(size);
            font
        };
        font.set_subpixel(true);
        if bold {
            font.set_embolden(true);
        }
        cache.insert(key, font.clone());
        font
    }

    fn match_font_family(
        &self,
        family: &str,
        style: FontStyle,
        required_cjk_glyphs: &[Unichar],
    ) -> Option<Typeface> {
        let family = family.trim();
        if family.is_empty() {
            return None;
        }

        if let Some(typeface) = self.match_custom_font_family(family, style, required_cjk_glyphs) {
            return Some(typeface);
        }

        let typeface = if family.eq_ignore_ascii_case("serif")
            || family.eq_ignore_ascii_case("sans-serif")
            || family.eq_ignore_ascii_case("monospace")
        {
            self.font_mgr.legacy_make_typeface(Some(family), style)
        } else {
            self.font_mgr.match_family_style(family, style)
        }?;

        if !typeface_supports_glyphs(&typeface, required_cjk_glyphs) {
            return None;
        }
        Some(typeface)
    }

    fn match_custom_font_family(
        &self,
        family: &str,
        style: FontStyle,
        required_cjk_glyphs: &[Unichar],
    ) -> Option<Typeface> {
        let requested = normalize_family_lookup(family);
        let candidates = self.custom_typefaces.get(&requested)?;
        let best = candidates
            .iter()
            .filter(|typeface| typeface_supports_glyphs(typeface, required_cjk_glyphs))
            .min_by_key(|typeface| font_style_distance(typeface.font_style(), style))?;
        Some(best.clone())
    }

    fn match_any_custom_font(
        &self,
        style: FontStyle,
        required_cjk_glyphs: &[Unichar],
    ) -> Option<Typeface> {
        self.custom_typefaces
            .values()
            .flatten()
            .filter(|typeface| typeface_supports_glyphs(typeface, required_cjk_glyphs))
            .min_by_key(|typeface| font_style_distance(typeface.font_style(), style))
            .cloned()
    }
}

#[derive(Default)]
struct SegmentLayers {
    slide_paths: Vec<NoteIdx>,
    notes: Vec<NoteIdx>,
    flicks: Vec<NoteIdx>,
    ticks: Vec<TickCommand>,
}

struct SegmentRaster {
    width: i32,
    height: i32,
    image: Image,
}

struct RenderIndex {
    active_notes: Vec<NoteIdx>,
    note_bars: Vec<Fraction>,
    slide_starts: Vec<NoteIdx>,
    slide_start_bars: Vec<Fraction>,
    next_ticks: Vec<NoteIdx>,
}

impl RenderIndex {
    fn new(active_notes: &[NoteIdx], notes: &[NoteData]) -> Self {
        let active_notes = active_notes.to_vec();
        let note_bars = active_notes
            .iter()
            .map(|&idx| notes[idx].bar())
            .collect::<Vec<_>>();
        let slide_starts = active_notes
            .iter()
            .copied()
            .filter(|&idx| {
                matches!(
                    notes[idx],
                    NoteData::Slide(_, ref slide)
                        if matches!(SlideType::from_i32(notes[idx].note_type()), Some(SlideType::Start))
                            && slide.head_idx != NO_NOTE
                )
            })
            .collect::<Vec<_>>();
        let slide_start_bars = slide_starts
            .iter()
            .map(|&idx| notes[idx].bar())
            .collect::<Vec<_>>();

        let mut next_ticks = vec![NO_NOTE; notes.len()];
        let tick_notes = active_notes
            .iter()
            .copied()
            .filter(|&idx| notes[idx].is_tick(notes) == Some(true))
            .collect::<Vec<_>>();
        let mut group_start = 0;
        while group_start < tick_notes.len() {
            let group_bar = notes[tick_notes[group_start]].bar();
            let mut group_end = group_start + 1;
            while group_end < tick_notes.len() && notes[tick_notes[group_end]].bar() == group_bar {
                group_end += 1;
            }

            let next_group_idx = tick_notes.get(group_end).copied();
            for &idx in &tick_notes[group_start..group_end] {
                next_ticks[idx] = next_group_idx.unwrap_or(idx);
            }
            group_start = group_end;
        }

        Self {
            active_notes,
            note_bars,
            slide_starts,
            slide_start_bars,
            next_ticks,
        }
    }
}

enum TickCommand {
    Short {
        bar: Fraction,
    },
    Text {
        note_idx: NoteIdx,
        next_idx: NoteIdx,
    },
}

struct SpeedLine {
    y: f64,
    speed: f64,
}

struct AmongCommand {
    kind: AmongKind,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy)]
enum AmongKind {
    LongAmong,
    LongAmongCritical,
    FrictionLong,
    FrictionFlick,
    FrictionCritical,
}

#[derive(Clone)]
struct NoteAssets {
    images: HashMap<String, Image>,
    sliced_notes: HashMap<NoteBodyKey, Image>,
}

impl NoteAssets {
    fn load(cfg: &DrawingConfig) -> Self {
        let mut images = HashMap::new();
        for name in note_asset_names() {
            let Some(path) = note_asset_path(cfg, &name) else {
                continue;
            };
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Some(image) = Image::from_encoded(Data::new_copy(&bytes)) else {
                continue;
            };
            images.insert(name, image);
        }

        let mut sliced_notes = HashMap::new();
        let note_height = cfg.lane_width as f64 / 64.0 * 56.0 * 2.0;
        for note_number in 0..=6 {
            let name = format!("notes_{note_number}");
            let Some(image) = images.get(&name) else {
                continue;
            };
            for width_units in 1..=13 {
                let width = cfg.lane_width as f64 * width_units as f64;
                if let Some(sliced) = render_sliced_note_image(image, width, note_height) {
                    sliced_notes.insert(
                        NoteBodyKey {
                            note_number,
                            width_units,
                        },
                        sliced,
                    );
                }
            }
        }

        Self {
            images,
            sliced_notes,
        }
    }

    fn get(&self, name: &str) -> Option<&Image> {
        self.images.get(name)
    }

    fn sliced_note(&self, note_number: i32, width_units: i32) -> Option<&Image> {
        self.sliced_notes.get(&NoteBodyKey {
            note_number,
            width_units,
        })
    }
}

fn build_font_manager(
    config: &DrawingConfig,
) -> Result<(FontMgr, SharedCustomTypefaces), SkiaDirectError> {
    let system_mgr = FontMgr::default();
    let font_paths = collect_font_paths(config);
    if font_paths.is_empty() {
        return Ok((system_mgr, Arc::new(HashMap::new())));
    }
    let cache_key = font_cache_key(&font_paths)?;

    if let Some(custom_typefaces) = CUSTOM_FONT_CACHE
        .lock()
        .expect("custom font cache lock poisoned")
        .get(&cache_key)
        .cloned()
    {
        return Ok((system_mgr, custom_typefaces));
    }

    let mut custom_typefaces = HashMap::<String, Vec<Typeface>>::new();
    for path in &font_paths {
        let bytes = fs::read(&path).map_err(|source| SkiaDirectError::Font {
            path: path.clone(),
            source,
        })?;
        for typeface in load_typefaces_from_data(&system_mgr, &bytes) {
            register_custom_typeface(&mut custom_typefaces, typeface);
        }
    }

    let custom_typefaces = Arc::new(custom_typefaces);
    let mut cache = CUSTOM_FONT_CACHE
        .lock()
        .expect("custom font cache lock poisoned");
    if cache.len() >= CUSTOM_FONT_CACHE_MAX_ENTRIES {
        cache.clear();
    }
    cache.insert(cache_key, Arc::clone(&custom_typefaces));

    Ok((system_mgr, custom_typefaces))
}

fn load_typefaces_from_data(font_mgr: &FontMgr, bytes: &[u8]) -> Vec<Typeface> {
    let mut faces = Vec::new();
    for ttc_index in 0..32 {
        let Some(typeface) = font_mgr.new_from_data(bytes, Some(ttc_index)) else {
            if ttc_index == 0 {
                return faces;
            }
            break;
        };
        faces.push(typeface);
    }
    faces
}

fn collect_font_paths(config: &DrawingConfig) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in &config.font_paths {
        let path = PathBuf::from(path);
        if is_font_path(&path) {
            paths.push(path);
        }
    }
    for dir in &config.font_dirs {
        collect_font_paths_from_dir(Path::new(dir), &mut paths);
    }
    paths.sort();
    paths.dedup();
    paths
}

fn collect_font_paths_from_dir(dir: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_paths_from_dir(&path, paths);
        } else if is_font_path(&path) {
            paths.push(path);
        }
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct FontFileKey {
    path: PathBuf,
    modified_nanos: Option<u128>,
    len: u64,
}

fn font_cache_key(font_paths: &[PathBuf]) -> Result<Vec<FontFileKey>, SkiaDirectError> {
    font_paths
        .iter()
        .map(|path| {
            let metadata = fs::metadata(path).map_err(|source| SkiaDirectError::Font {
                path: path.clone(),
                source,
            })?;
            Ok(FontFileKey {
                path: path.clone(),
                modified_nanos: metadata.modified().ok().and_then(system_time_nanos),
                len: metadata.len(),
            })
        })
        .collect()
}

fn system_time_nanos(time: SystemTime) -> Option<u128> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn is_font_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("ttf")
                || extension.eq_ignore_ascii_case("otf")
                || extension.eq_ignore_ascii_case("ttc")
        })
}

fn register_custom_typeface(
    custom_typefaces: &mut HashMap<String, Vec<Typeface>>,
    typeface: Typeface,
) {
    let mut names = typeface
        .new_family_name_iterator()
        .map(|localized| localized.string)
        .collect::<Vec<_>>();
    names.push(typeface.family_name());
    if let Some(post_script_name) = typeface.post_script_name() {
        names.push(post_script_name);
    }

    for name in names {
        let name = normalize_family_lookup(&name);
        if !name.is_empty() {
            custom_typefaces
                .entry(name)
                .or_default()
                .push(typeface.clone());
        }
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct NoteBodyKey {
    note_number: i32,
    width_units: i32,
}

fn note_asset_names() -> Vec<String> {
    let mut names = Vec::new();
    for note_number in 0..=6 {
        names.push(format!("notes_{note_number}"));
    }
    names.extend(
        [
            "notes_long_among",
            "notes_long_among_crtcl",
            "notes_friction_among_long",
            "notes_friction_among_flick",
            "notes_friction_among_crtcl",
        ]
        .into_iter()
        .map(str::to_string),
    );
    for critical in [false, true] {
        for width in 1..=6 {
            for diagonal in [false, true] {
                names.push(format!(
                    "notes_flick_arrow{}_{:02}{}",
                    if critical { "_crtcl" } else { "" },
                    width,
                    if diagonal { "_diagonal" } else { "" },
                ));
            }
        }
    }
    names
}

fn note_asset_path(cfg: &DrawingConfig, name: &str) -> Option<PathBuf> {
    if cfg.note_host.starts_with("http://") || cfg.note_host.starts_with("https://") {
        return None;
    }
    let host = cfg
        .note_host
        .strip_prefix("file://")
        .unwrap_or(&cfg.note_host);
    let path = Path::new(host).join(format!("{}.{}", name, cfg.note_asset_extension));
    if path.exists() { Some(path) } else { None }
}

fn should_render_segments_parallel(layout: &Layout) -> bool {
    layout.segments.len() > 1
        && std::thread::available_parallelism()
            .map(|threads| threads.get() > 1)
            .unwrap_or(false)
}

fn segment_score(source: &Score) -> Score {
    Score {
        meta: source.meta.clone(),
        notes: Vec::new(),
        active_notes: source.active_notes.clone(),
        events: source.events.clone(),
        timed_events_cache: source.timed_events_cache.clone(),
        time_cache: HashMap::new(),
        time_f64_cache: HashMap::new(),
    }
}

fn draw_segment_image(
    canvas: &skia_safe::Canvas,
    raster: &SegmentRaster,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), SkiaDirectError> {
    canvas.save();
    canvas.clip_rect(
        Rect::from_xywh(as_f32(x), as_f32(y), as_f32(width), as_f32(height)),
        None,
        Some(true),
    );
    draw_image(
        canvas,
        &raster.image,
        x,
        y,
        raster.width as f64,
        raster.height as f64,
    );
    canvas.restore();
    Ok(())
}

#[derive(Clone, Copy)]
enum TextAnchor {
    Start,
    End,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct FontKey {
    size_bits: u32,
    weight: i32,
    family_hash: u64,
    cjk_hash: u64,
}

impl FontKey {
    fn new(
        size: f32,
        weight: i32,
        font_families: Option<&[String]>,
        required_cjk_glyphs: &[Unichar],
    ) -> Self {
        Self {
            size_bits: size.to_bits(),
            weight,
            family_hash: hash_font_families(font_families),
            cjk_hash: hash_required_glyphs(required_cjk_glyphs),
        }
    }
}

#[derive(Clone, Copy)]
struct TextDefaults {
    color: RGBA,
    size: f32,
    weight: i32,
}

impl TextDefaults {
    fn new(color: RGBA, size: f32, weight: i32) -> Self {
        Self {
            color,
            size,
            weight,
        }
    }
}

fn note_visible(
    note: &NoteData,
    arena: &[NoteData],
    bar_start_f: Fraction,
    bar_stop_f: Fraction,
) -> bool {
    if note.is_slide() {
        let Some(slide) = note.as_slide() else {
            return false;
        };
        let head_idx = slide.head_idx;
        if head_idx == NO_NOTE {
            return false;
        }
        let mut cur_idx = head_idx;
        let mut before = false;
        loop {
            let cur = &arena[cur_idx];
            if let Some(s) = cur.as_slide()
                && !s.is_path(cur.note_type())
            {
                if s.next_idx == NO_NOTE {
                    break;
                }
                cur_idx = s.next_idx;
                continue;
            }

            let bar = arena[cur_idx].bar();
            if bar_start_f - Fraction::from_integer(1) <= bar
                && bar < bar_stop_f + Fraction::from_integer(1)
            {
                return true;
            } else if bar < bar_start_f - Fraction::from_integer(1) {
                before = true;
            } else if before && bar_stop_f + Fraction::from_integer(1) < bar {
                return true;
            }

            if let Some(s) = arena[cur_idx].as_slide() {
                if s.next_idx == NO_NOTE {
                    break;
                }
                cur_idx = s.next_idx;
            } else {
                break;
            }
        }
        false
    } else {
        let bar = note.bar();
        bar_start_f - Fraction::from_integer(1) <= bar
            && bar < bar_stop_f + Fraction::from_integer(1)
    }
}

fn bezier_coordinates(
    cfg: &DrawingConfig,
    score: &mut Score,
    arena: &[NoteData],
    idx0: NoteIdx,
    idx1: NoteIdx,
    bar_stop: Fraction,
) -> (BezierPoints, BezierPoints) {
    let slide_0 = &arena[idx0];
    let slide_1 = &arena[idx1];
    let y_0 = cfg.time_height * score.get_time_delta_f64(slide_0.bar(), bar_stop)
        + cfg.time_padding as f64;
    let y_1 = cfg.time_height * score.get_time_delta_f64(slide_1.bar(), bar_stop)
        + cfg.time_padding as f64;

    let ease_in = slide_0
        .as_slide()
        .and_then(|s| {
            if s.directional_idx != NO_NOTE {
                DirectionalType::from_i32(arena[s.directional_idx].note_type())
                    .filter(|dt| matches!(dt, DirectionalType::Down))
            } else {
                None
            }
        })
        .is_some();
    let ease_out = slide_0
        .as_slide()
        .and_then(|s| {
            if s.directional_idx != NO_NOTE {
                DirectionalType::from_i32(arena[s.directional_idx].note_type()).filter(|dt| {
                    matches!(dt, DirectionalType::LowerLeft | DirectionalType::LowerRight)
                })
            } else {
                None
            }
        })
        .is_some();

    let is_decoration = slide_0.as_slide().map(|s| s.decoration).unwrap_or(false);
    let spp = if is_decoration {
        0.0
    } else {
        cfg.slide_path_padding
    };
    let l0_x = cfg.lane_width as f64 * (slide_0.lane() - 2) as f64 + cfg.lane_padding as f64 - spp;
    let l1_x = cfg.lane_width as f64 * (slide_1.lane() - 2) as f64 + cfg.lane_padding as f64 - spp;
    let r0_x = cfg.lane_width as f64 * (slide_0.lane() - 2 + slide_0.width()) as f64
        + cfg.lane_padding as f64
        + spp;
    let r1_x = cfg.lane_width as f64 * (slide_1.lane() - 2 + slide_1.width()) as f64
        + cfg.lane_padding as f64
        + spp;
    let mid_y = (y_0 + y_1) / 2.0;

    (
        [
            (l0_x, y_0),
            (l0_x, if ease_in { mid_y } else { y_0 }),
            (l1_x, if ease_out { mid_y } else { y_1 }),
            (l1_x, y_1),
        ],
        [
            (r0_x, y_0),
            (r0_x, if ease_in { mid_y } else { y_0 }),
            (r1_x, if ease_out { mid_y } else { y_1 }),
            (r1_x, y_1),
        ],
    )
}

fn binary_solution_for_x(y: f64, curve: &BezierPoints) -> f64 {
    binary_solution_for_x_inner(y, curve, 0.0, 1.0, 0.1, 100)
}

fn binary_solution_for_x_inner(
    y: f64,
    curve: &BezierPoints,
    start: f64,
    end: f64,
    epsilon: f64,
    max_depth: u32,
) -> f64 {
    let t = (start + end) / 2.0;
    let t1 = 1.0 - t;
    let px = curve[0].0 * t1 * t1 * t1
        + curve[1].0 * 3.0 * t1 * t1 * t
        + curve[2].0 * 3.0 * t1 * t * t
        + curve[3].0 * t * t * t;
    let py = curve[0].1 * t1 * t1 * t1
        + curve[1].1 * 3.0 * t1 * t1 * t
        + curve[2].1 * 3.0 * t1 * t * t
        + curve[3].1 * t * t * t;

    if (py - y).abs() < epsilon || max_depth == 0 {
        px
    } else if py > y {
        binary_solution_for_x_inner(y, curve, t, end, epsilon, max_depth - 1)
    } else {
        binary_solution_for_x_inner(y, curve, start, t, epsilon, max_depth - 1)
    }
}

fn draw_vector_note_body(
    canvas: &skia_safe::Canvas,
    note_number: i32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) {
    let p = note_palette(note_number);
    let long_bar = matches!(note_number, 4..=6);
    let body_x = x + width * 0.08;
    let body_y = y + if long_bar {
        height * 0.28
    } else {
        height * 0.22
    };
    let body_w = width * 0.84;
    let body_h = if long_bar {
        height * 0.46
    } else {
        height * 0.56
    };
    let radius = body_h * 0.16;

    draw_round_rect(
        canvas,
        body_x - body_h * 0.28,
        body_y - body_h * 0.28,
        body_w + body_h * 0.56,
        body_h + body_h * 0.56,
        radius * 1.6,
        p.glow.with_alpha_factor(0.18),
    );
    draw_round_rect(
        canvas,
        body_x - body_h * 0.12,
        body_y - body_h * 0.12,
        body_w + body_h * 0.24,
        body_h + body_h * 0.24,
        radius * 1.25,
        p.glow.with_alpha_factor(0.24),
    );
    draw_round_rect(canvas, body_x, body_y, body_w, body_h, radius, p.fill);

    if long_bar {
        draw_round_rect(
            canvas,
            body_x + body_h * 0.10,
            body_y + body_h * 0.62,
            body_w - body_h * 0.20,
            body_h * 0.20,
            body_h * 0.10,
            p.accent.with_alpha_factor(0.48),
        );
    } else {
        draw_round_rect(
            canvas,
            body_x + body_h * 0.14,
            body_y + body_h * 0.16,
            body_w - body_h * 0.28,
            body_h * 0.56,
            radius * 0.78,
            p.inner.with_alpha_factor(0.72),
        );
        let cap_w = body_h * 0.32;
        let cap_h = body_h * 0.32;
        for cap_x in [body_x + body_h * 0.16, body_x + body_w - body_h * 0.48] {
            draw_round_rect(
                canvas,
                cap_x,
                body_y + body_h * 0.34,
                cap_w,
                cap_h,
                cap_h * 0.16,
                p.accent.with_alpha_factor(0.68),
            );
        }
    }

    let paint = stroke_paint(
        p.stroke.with_alpha_factor(0.82),
        (height * 0.045).max(1.0) as f32,
    );
    canvas.draw_round_rect(
        &rect(body_x, body_y, body_w, body_h),
        as_f32(radius),
        as_f32(radius),
        &paint,
    );
}

fn draw_vector_flick(
    canvas: &skia_safe::Canvas,
    is_critical: bool,
    is_diagonal: bool,
    flip_right: bool,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) {
    let note_number = if is_critical { 0 } else { 3 };
    let p = note_palette(note_number);

    canvas.save();
    if flip_right {
        canvas.translate((as_f32(x + width), as_f32(y)));
        canvas.scale((-1.0, 1.0));
    } else {
        canvas.translate((as_f32(x), as_f32(y)));
    }

    let shadow = [
        (width * 0.20, height * 0.62),
        (width * 0.80, height * 0.62),
        (width * 0.88, height * 0.98),
        (width * 0.12, height * 0.98),
    ];
    draw_polygon(
        canvas,
        &shadow,
        fill_paint(RGBA::WHITE.with_alpha_factor(0.28)),
    );

    let (arrow, rotation) = if is_diagonal {
        (
            [
                (width * 0.50, height * 0.06),
                (width * 0.88, height * 0.30),
                (width * 0.96, height * 0.64),
                (width * 0.62, height * 0.56),
                (width * 0.50, height * 0.74),
                (width * 0.38, height * 0.56),
                (width * 0.04, height * 0.64),
                (width * 0.12, height * 0.30),
            ],
            Some(-18.0),
        )
    } else {
        (
            [
                (width * 0.50, height * 0.08),
                (width * 0.88, height * 0.34),
                (width * 0.94, height * 0.70),
                (width * 0.61, height * 0.56),
                (width * 0.50, height * 0.72),
                (width * 0.39, height * 0.56),
                (width * 0.06, height * 0.70),
                (width * 0.12, height * 0.34),
            ],
            None,
        )
    };

    canvas.save();
    if let Some(angle) = rotation {
        canvas.translate((as_f32(width * 0.5), as_f32(height * 0.48)));
        canvas.rotate(angle, None);
        canvas.translate((as_f32(-width * 0.5), as_f32(-height * 0.48)));
    }
    draw_polygon_stroke(
        canvas,
        &arrow,
        p.glow.with_alpha_factor(0.18),
        width.max(height) * 0.09,
    );
    draw_polygon(canvas, &arrow, fill_paint(p.fill));
    draw_polygon_stroke(canvas, &arrow, p.stroke, width.max(height) * 0.04);
    canvas.restore();
    canvas.restore();
}

fn draw_vector_among(
    canvas: &skia_safe::Canvas,
    kind: AmongKind,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) {
    let palette = match kind {
        AmongKind::LongAmong | AmongKind::FrictionLong => note_palette(1),
        AmongKind::FrictionFlick => note_palette(3),
        AmongKind::LongAmongCritical | AmongKind::FrictionCritical => note_palette(0),
    };

    let cx = x + width * 0.5;
    let top = y + height * 0.16;
    let mid = y + height * 0.52;
    let bottom = y + height * 0.82;
    let left = x + width * 0.08;
    let right = x + width * 0.92;

    let shadow = [
        (left, mid),
        (cx, y + height * 0.94),
        (right, mid),
        (cx, bottom),
    ];
    let diamond = [(cx, top), (right, mid), (cx, bottom), (left, mid)];
    let left_face = [(cx, top), (cx, bottom), (left, mid)];
    let right_face = [(cx, top), (right, mid), (cx, bottom)];

    draw_polygon(
        canvas,
        &shadow,
        fill_paint(RGBA::rgb(0x11, 0x18, 0x20).with_alpha_factor(0.22)),
    );
    draw_polygon_stroke(
        canvas,
        &diamond,
        palette.glow.with_alpha_factor(0.18),
        width.max(height) * 0.18,
    );
    draw_polygon(canvas, &left_face, fill_paint(palette.inner));
    draw_polygon(
        canvas,
        &right_face,
        fill_paint(palette.fill.with_alpha_factor(0.72)),
    );
    draw_polyline(
        canvas,
        &[(left, mid), (cx, top), (right, mid)],
        palette.stroke,
        width.max(height) * 0.08,
    );
    draw_polyline(
        canvas,
        &[(left, mid), (cx, bottom), (right, mid)],
        palette.accent.with_alpha_factor(0.72),
        width.max(height) * 0.05,
    );
}

fn draw_round_rect(
    canvas: &skia_safe::Canvas,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
    color: RGBA,
) {
    canvas.draw_round_rect(
        &rect(x, y, width, height),
        as_f32(radius),
        as_f32(radius),
        &fill_paint(color),
    );
}

fn draw_polygon(canvas: &skia_safe::Canvas, points: &[(f64, f64)], paint: Paint) {
    if points.is_empty() {
        return;
    }
    let mut path = PathBuilder::new();
    path.move_to((as_f32(points[0].0), as_f32(points[0].1)));
    for point in &points[1..] {
        path.line_to((as_f32(point.0), as_f32(point.1)));
    }
    path.close();
    let path = path.detach();
    canvas.draw_path(&path, &paint);
}

fn draw_polygon_stroke(canvas: &skia_safe::Canvas, points: &[(f64, f64)], color: RGBA, width: f64) {
    draw_polygon(canvas, points, stroke_paint(color, as_f32(width)));
}

fn draw_polyline(canvas: &skia_safe::Canvas, points: &[(f64, f64)], color: RGBA, width: f64) {
    if points.is_empty() {
        return;
    }
    let mut path = PathBuilder::new();
    path.move_to((as_f32(points[0].0), as_f32(points[0].1)));
    for point in &points[1..] {
        path.line_to((as_f32(point.0), as_f32(point.1)));
    }
    let path = path.detach();
    canvas.draw_path(&path, &stroke_paint(color, as_f32(width)));
}

fn draw_sliced_note_image(
    canvas: &skia_safe::Canvas,
    image: &Image,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) {
    let src_w = image.width() as f64;
    let src_h = image.height() as f64;
    if src_w <= 0.0 || src_h <= 0.0 {
        return;
    }

    let src_cap = (src_w * 32.0 / 112.0).min(src_w / 2.0);
    let dst_cap = (height * 32.0 / 56.0).min(width / 2.0);
    let src_mid_w = (src_w - src_cap * 2.0).max(0.0);
    let dst_mid_w = (width - dst_cap * 2.0).max(0.0);

    if dst_cap > 0.0 {
        draw_image_src_dst(
            canvas,
            image,
            rect(0.0, 0.0, src_cap, src_h),
            rect(x, y, dst_cap, height),
        );
        draw_image_src_dst(
            canvas,
            image,
            rect(src_w - src_cap, 0.0, src_cap, src_h),
            rect(x + width - dst_cap, y, dst_cap, height),
        );
    }
    if src_mid_w > 0.0 && dst_mid_w > 0.0 {
        draw_image_src_dst(
            canvas,
            image,
            rect(src_cap, 0.0, src_mid_w, src_h),
            rect(x + dst_cap, y, dst_mid_w, height),
        );
    }
}

fn render_sliced_note_image(image: &Image, width: f64, height: f64) -> Option<Image> {
    let width_px = round_even(width) as i32;
    let height_px = round_even(height) as i32;
    if width_px <= 0 || height_px <= 0 {
        return None;
    }

    let mut surface = surfaces::raster_n32_premul((width_px, height_px))?;
    let canvas = surface.canvas();
    canvas.clear(Color::TRANSPARENT);
    draw_sliced_note_image(canvas, image, 0.0, 0.0, width, height);
    Some(surface.image_snapshot())
}

fn draw_image(canvas: &skia_safe::Canvas, image: &Image, x: f64, y: f64, width: f64, height: f64) {
    let dst = rect(x, y, width, height);
    let paint = image_paint();
    let sampling = SamplingOptions::from(FilterMode::Linear);
    canvas.draw_image_rect_with_sampling_options(image, None, &dst, sampling, &paint);
}

fn draw_image_src_dst(canvas: &skia_safe::Canvas, image: &Image, src: Rect, dst: Rect) {
    let paint = image_paint();
    let sampling = SamplingOptions::from(FilterMode::Linear);
    canvas.draw_image_rect_with_sampling_options(
        image,
        Some((&src, skia_safe::canvas::SrcRectConstraint::Fast)),
        &dst,
        sampling,
        &paint,
    );
}

fn image_paint() -> Paint {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint
}

fn fill_paint(color: RGBA) -> Paint {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);
    paint.set_color(color.to_color());
    paint
}

fn stroke_paint(color: RGBA, width: f32) -> Paint {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Stroke);
    paint.set_stroke_width(width);
    paint.set_color(color.to_color());
    paint
}

fn point(point: (f64, f64)) -> Point {
    Point::new(as_f32(point.0), as_f32(point.1))
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect::from_xywh(as_f32(x), as_f32(y), as_f32(width), as_f32(height))
}

fn as_f32(value: f64) -> f32 {
    value as f32
}

#[derive(Debug, Clone, Copy)]
struct NotePalette {
    glow: RGBA,
    fill: RGBA,
    stroke: RGBA,
    accent: RGBA,
    inner: RGBA,
}

fn note_palette(note_number: i32) -> NotePalette {
    match note_number {
        0 | 5 => NotePalette {
            glow: RGBA::rgb(0xff, 0xd8, 0x4a),
            fill: RGBA::rgb(0xff, 0xf5, 0x8a),
            stroke: RGBA::rgb(0xff, 0xf9, 0xd9),
            accent: RGBA::rgb(0xff, 0xb7, 0x33),
            inner: RGBA::rgb(0xff, 0xfb, 0xd6),
        },
        1 | 4 => NotePalette {
            glow: RGBA::rgb(0x3c, 0xf0, 0xa2),
            fill: RGBA::rgb(0x66, 0xf4, 0xbd),
            stroke: RGBA::rgb(0xe4, 0xff, 0xf6),
            accent: RGBA::rgb(0x22, 0xd9, 0x89),
            inner: RGBA::rgb(0xdf, 0xff, 0xf2),
        },
        2 => NotePalette {
            glow: RGBA::rgb(0x68, 0xe2, 0xff),
            fill: RGBA::rgb(0x90, 0xef, 0xff),
            stroke: RGBA::rgb(0xf2, 0xf4, 0xff),
            accent: RGBA::rgb(0x75, 0x6d, 0xff),
            inner: RGBA::rgb(0xea, 0xff, 0xff),
        },
        _ => NotePalette {
            glow: RGBA::rgb(0xff, 0x70, 0xbc),
            fill: RGBA::rgb(0xff, 0x8f, 0xcc),
            stroke: RGBA::rgb(0xff, 0xf0, 0xfa),
            accent: RGBA::rgb(0xee, 0x5a, 0xaa),
            inner: RGBA::rgb(0xff, 0xe7, 0xf5),
        },
    }
}

#[derive(Debug, Clone, Copy)]
struct RGBA {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

impl RGBA {
    const WHITE: Self = Self::rgb(0xff, 0xff, 0xff);
    const BLACK: Self = Self::rgb(0x00, 0x00, 0x00);

    const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 0xff }
    }

    const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    fn with_alpha_factor(self, factor: f64) -> Self {
        Self {
            a: ((self.a as f64 * factor).round().clamp(0.0, 255.0)) as u8,
            ..self
        }
    }

    fn to_color(self) -> Color {
        Color::from_argb(self.a, self.r, self.g, self.b)
    }
}

#[derive(Clone, Default)]
struct CssRuleStyle {
    fill: Option<RGBA>,
    stroke: Option<RGBA>,
    stroke_width: Option<f32>,
    font_size: Option<f32>,
    font_weight: Option<i32>,
    font_families: Option<Vec<String>>,
}

#[derive(Clone)]
struct CssStyles {
    rules: HashMap<String, CssRuleStyle>,
}

impl CssStyles {
    fn parse(css: &str) -> Self {
        let css = strip_css_comments(css);
        let mut rules = HashMap::<String, CssRuleStyle>::new();
        let mut cursor = 0usize;
        while let Some(open_rel) = css[cursor..].find('{') {
            let open = cursor + open_rel;
            let Some(close_rel) = css[open + 1..].find('}') else {
                break;
            };
            let close = open + 1 + close_rel;
            let selectors = css[cursor..open].trim();
            let body = &css[open + 1..close];
            let parsed = parse_css_body(body);
            for selector in selectors.split(',') {
                let selector = selector.trim();
                let Some(class_name) = selector.strip_prefix('.') else {
                    continue;
                };
                let class_name = class_name.split_whitespace().next().unwrap_or("").trim();
                if class_name.is_empty() {
                    continue;
                }
                let entry = rules.entry(class_name.to_string()).or_default();
                entry.merge(&parsed);
            }
            cursor = close + 1;
        }
        Self { rules }
    }

    fn get(&self, class_name: &str) -> CssRuleStyle {
        self.rules.get(class_name).cloned().unwrap_or_default()
    }
}

impl CssRuleStyle {
    fn merge(&mut self, other: &CssRuleStyle) {
        if other.fill.is_some() {
            self.fill = other.fill;
        }
        if other.stroke.is_some() {
            self.stroke = other.stroke;
        }
        if other.stroke_width.is_some() {
            self.stroke_width = other.stroke_width;
        }
        if other.font_size.is_some() {
            self.font_size = other.font_size;
        }
        if other.font_weight.is_some() {
            self.font_weight = other.font_weight;
        }
        if other.font_families.is_some() {
            self.font_families.clone_from(&other.font_families);
        }
    }
}

fn parse_css_body(body: &str) -> CssRuleStyle {
    let mut style = CssRuleStyle::default();
    for declaration in body.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        match name {
            "fill" => style.fill = parse_color(value),
            "stroke" => style.stroke = parse_color(value),
            "stroke-width" => style.stroke_width = parse_css_number(value),
            "font-size" => style.font_size = parse_css_number(value),
            "font-weight" => style.font_weight = value.parse::<i32>().ok(),
            "font-family" => style.font_families = parse_font_families(value),
            _ => {}
        }
    }
    style
}

fn parse_font_families(value: &str) -> Option<Vec<String>> {
    let families = split_css_list(value)
        .into_iter()
        .filter_map(|family| normalize_font_family(&family))
        .collect::<Vec<_>>();
    if families.is_empty() {
        None
    } else {
        Some(families)
    }
}

fn split_css_list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in value.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(quote_char) = quote {
            if ch == quote_char {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ',' => {
                items.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        items.push(current.trim().to_string());
    }
    items
}

fn normalize_font_family(value: &str) -> Option<String> {
    let family = value.trim();
    if family.is_empty()
        || family.eq_ignore_ascii_case("inherit")
        || family.eq_ignore_ascii_case("initial")
    {
        return None;
    }
    Some(family.to_string())
}

fn parse_css_number(value: &str) -> Option<f32> {
    value
        .trim()
        .trim_end_matches("px")
        .trim()
        .parse::<f32>()
        .ok()
}

fn hash_font_families(font_families: Option<&[String]>) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    if let Some(families) = font_families {
        for family in families {
            for byte in family.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash ^= 0xff;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

fn hash_required_glyphs(glyphs: &[Unichar]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for glyph in glyphs {
        for byte in glyph.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

fn normalize_family_lookup(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn font_style_distance(candidate: FontStyle, requested: FontStyle) -> i32 {
    ((*candidate.weight() - *requested.weight()).abs() * 100)
        + ((*candidate.width() - *requested.width()).abs() * 10)
        + i32::from(candidate.slant() != requested.slant())
}

fn required_cjk_glyphs(text: &str) -> Vec<Unichar> {
    let mut glyphs = text
        .chars()
        .filter(|ch| is_cjk_char(*ch))
        .map(|ch| ch as Unichar)
        .collect::<Vec<_>>();
    glyphs.sort_unstable();
    glyphs.dedup();
    glyphs
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2e80..=0x2eff
            | 0x3000..=0x303f
            | 0x3040..=0x30ff
            | 0x31f0..=0x31ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0xff00..=0xffef
            | 0x20000..=0x2ebef
            | 0x30000..=0x3134f
    )
}

fn typeface_supports_glyphs(typeface: &Typeface, required_glyphs: &[Unichar]) -> bool {
    required_glyphs
        .iter()
        .all(|glyph| typeface.unichar_to_glyph(*glyph) != 0)
}

fn parse_color(value: &str) -> Option<RGBA> {
    let value = value.trim();
    match value {
        "black" => return Some(RGBA::BLACK),
        "white" => return Some(RGBA::WHITE),
        "transparent" | "none" => return None,
        _ => {}
    }
    let hex = value.strip_prefix('#')?;
    let expand = |c: char| -> Option<u8> {
        let d = c.to_digit(16)? as u8;
        Some((d << 4) | d)
    };
    match hex.len() {
        3 => {
            let mut chars = hex.chars();
            Some(RGBA::rgb(
                expand(chars.next()?)?,
                expand(chars.next()?)?,
                expand(chars.next()?)?,
            ))
        }
        4 => {
            let mut chars = hex.chars();
            Some(RGBA::rgba(
                expand(chars.next()?)?,
                expand(chars.next()?)?,
                expand(chars.next()?)?,
                expand(chars.next()?)?,
            ))
        }
        6 => Some(RGBA::rgb(
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        )),
        8 => Some(RGBA::rgba(
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            u8::from_str_radix(&hex[6..8], 16).ok()?,
        )),
        _ => None,
    }
}

fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];
        if let Some(end) = after_start.find("*/") {
            rest = &after_start[end + 2..];
        } else {
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn local_image_path(value: &str) -> Option<PathBuf> {
    if value.starts_with("http://") || value.starts_with("https://") {
        return None;
    }
    let value = value.strip_prefix("file://").unwrap_or(value);
    let path = Path::new(value);
    if path.exists() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

fn round_even(v: f64) -> i64 {
    let floor = v.floor();
    let diff = v - floor;
    if diff > 0.5 {
        (floor + 1.0) as i64
    } else if diff < 0.5 {
        floor as i64
    } else {
        let floor_i = floor as i64;
        if floor_i.rem_euclid(2) == 0 {
            floor_i
        } else {
            floor_i + 1
        }
    }
}

fn format_g(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    let abs_v = v.abs();
    let exp = abs_v.log10().floor() as i32;

    if (-4..6).contains(&exp) {
        let decimals = (5 - exp).max(0) as usize;
        let s = format!("{:.prec$}", v, prec = decimals);
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    } else {
        let mantissa = v / 10.0_f64.powi(exp);
        let s = format!("{mantissa:.5}");
        let m = s.trim_end_matches('0').trim_end_matches('.');
        if exp >= 0 {
            format!("{m}e+{exp:02}")
        } else {
            format!("{m}e-{:02}", -exp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_css_body, parse_font_families};

    #[test]
    fn parses_unquoted_rodin_font_family() {
        assert_eq!(
            parse_font_families("FOT-RodinNTLG Pro DB"),
            Some(vec!["FOT-RodinNTLG Pro DB".to_string()])
        );
    }

    #[test]
    fn parses_quoted_font_family_list() {
        assert_eq!(
            parse_font_families(
                "\"ヒラギノ角ゴ Pro W3\", \"Hiragino Kaku Gothic Pro\", Osaka, sans-serif",
            ),
            Some(vec![
                "ヒラギノ角ゴ Pro W3".to_string(),
                "Hiragino Kaku Gothic Pro".to_string(),
                "Osaka".to_string(),
                "sans-serif".to_string(),
            ])
        );
    }

    #[test]
    fn keeps_font_family_from_css_rule() {
        let style = parse_css_body(
            r#"
            font-family: FOT-RodinNTLG Pro DB;
            font-size: 36px;
            font-weight: 700;
            "#,
        );
        assert_eq!(
            style.font_families,
            Some(vec!["FOT-RodinNTLG Pro DB".to_string()])
        );
        assert_eq!(style.font_size, Some(36.0));
        assert_eq!(style.font_weight, Some(700));
    }
}
