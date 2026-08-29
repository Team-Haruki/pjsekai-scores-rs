use std::fmt::Write;

use crate::fraction::Fraction;
use crate::lyric::Lyric;
use crate::notes::directional::DirectionalType;
use crate::notes::event::Event;
use crate::notes::slide::SlideType;
use crate::notes::{NO_NOTE, NoteData, NoteIdx};
use crate::score::Score;

const DEFAULT_CSS: &str = include_str!("../css/default.css");

/// Cover object for skill/fever overlays
#[derive(Debug, Clone)]
pub enum CoverObject {
    Rect {
        bar_from: Fraction,
        css_class: String,
        bar_to: Fraction,
    },
    Text {
        bar_from: Fraction,
        css_class: String,
        text: String,
    },
}

/// Configuration for the drawing
pub struct DrawingConfig {
    pub n_lanes: i32,
    pub lane_width: i32,
    pub time_height: f64,
    pub note_size: i32,
    pub flick_height: i32,
    pub lane_padding: i32,
    pub time_padding: i32,
    pub slide_path_padding: f64,
    pub meta_size: i32,
    pub tick_length: i32,
    pub tick_2_length: i32,
    pub note_host: String,
    pub note_asset_extension: String,
    pub font_paths: Vec<String>,
    pub font_dirs: Vec<String>,
    /// Generator name shown in the SVG subtitle (default: "HarukiBot NEO")
    pub generator: String,
}

impl Default for DrawingConfig {
    fn default() -> Self {
        DrawingConfig {
            n_lanes: 12,
            lane_width: 16,
            time_height: 360.0,
            note_size: 16,
            flick_height: 24,
            lane_padding: 40,
            time_padding: 32,
            slide_path_padding: -1.0,
            meta_size: 192,
            tick_length: 24,
            tick_2_length: 8,
            note_host: "https://asset3.pjsekai.moe/live/note/custom01".to_string(),
            note_asset_extension: "png".to_string(),
            font_paths: Vec::new(),
            font_dirs: Vec::new(),
            generator: "HarukiBot NEO".to_string(),
        }
    }
}

/// Main drawing struct that generates SVG from a Score
pub struct Drawing {
    pub config: DrawingConfig,
    pub style_sheet: String,
    pub skill: bool,
    pub music_meta: Option<MusicMeta>,
    pub special_cover_objects: Vec<CoverObject>,
}

/// Music metadata for skill score display
#[derive(Debug, Clone)]
pub struct MusicMeta {
    pub fever_end_time: f64,
    pub fever_score: f64,
    pub skill_score_solo: Vec<f64>,
    pub skill_score_multi: Vec<f64>,
}

type BezierPoints = [(f64, f64); 4];

#[derive(Debug, Clone, Copy)]
enum AmongKind {
    LongAmong,
    LongAmongCritical,
    FrictionLong,
    FrictionFlick,
    FrictionCritical,
}

impl Drawing {
    pub fn new(
        note_host: Option<String>,
        style_sheet: Option<String>,
        skill: bool,
        music_meta: Option<MusicMeta>,
        _target_segment_seconds: Option<f64>,
        generator: Option<String>,
    ) -> Self {
        let mut config = DrawingConfig::default();
        if let Some(nh) = note_host {
            config.note_host = nh;
        }
        if let Some(g) = generator {
            config.generator = g;
        }

        let mut css = DEFAULT_CSS.to_string();
        if let Some(extra) = style_sheet {
            css.push('\n');
            css.push_str(&extra);
        }

        Drawing {
            config,
            style_sheet: css,
            skill,
            music_meta,
            special_cover_objects: Vec::new(),
        }
    }

    pub fn set_note_asset_extension(&mut self, extension: impl Into<String>) {
        self.config.note_asset_extension = normalize_note_asset_extension(extension.into());
    }

    pub fn set_style_sheet(&mut self, style_sheet: Option<String>) {
        let mut css = DEFAULT_CSS.to_string();
        if let Some(extra) = style_sheet {
            css.push('\n');
            css.push_str(&extra);
        }
        self.style_sheet = css;
    }

    pub fn set_font_paths<I, S>(&mut self, paths: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.font_paths = paths
            .into_iter()
            .map(Into::into)
            .filter(|path| !path.trim().is_empty())
            .collect();
    }

    pub fn add_font_path(&mut self, path: impl Into<String>) {
        let path = path.into();
        if !path.trim().is_empty() {
            self.config.font_paths.push(path);
        }
    }

    pub fn set_font_dirs<I, S>(&mut self, dirs: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.font_dirs = dirs
            .into_iter()
            .map(Into::into)
            .filter(|dir| !dir.trim().is_empty())
            .collect();
    }

    pub fn add_font_dir(&mut self, dir: impl Into<String>) {
        let dir = dir.into();
        if !dir.trim().is_empty() {
            self.config.font_dirs.push(dir);
        }
    }

    /// Generate a complete SVG string from a score
    pub fn svg(&mut self, score: &mut Score, lyric: Option<&Lyric>) -> String {
        let n_bars = score
            .active_notes
            .last()
            .map(|&idx| score.notes[idx].bar().ceil() as i32)
            .unwrap_or(0);

        // Build skill cover objects (mutates self)
        if self.skill {
            self.build_skill_covers(score);
        }

        // Now safe to borrow config immutably
        let cfg = &self.config;

        let mut segments: Vec<(i32, i32)> = Vec::new(); // (bar_start, bar_stop)
        let mut bar_start = 0;
        let mut event = Event::new(Fraction::zero());
        event.bpm = Some(Fraction::from_integer(120));
        event.bar_length = Some(Fraction::from_integer(4));
        event.sentence_length = Some(4);

        for i in 0..=n_bars {
            let e = score.get_event(Fraction::from_integer(i as i64));
            let current_sentence_length = e.sentence_length.unwrap_or(4);
            let previous_sentence_length = event.sentence_length.unwrap_or(4);

            if bar_start != i
                && (e.section != event.section
                    || current_sentence_length != previous_sentence_length
                    || i == bar_start + previous_sentence_length
                    || i == n_bars)
            {
                segments.push((bar_start, i));
                bar_start = i;
            }

            event.merge_from(&e);
        }

        // Generate each segment SVG
        let mut segment_svgs: Vec<(String, f64, f64)> = Vec::new(); // (svg, width, height)
        let mut total_width: f64 = 0.0;
        let mut max_height: f64 = 0.0;

        for (start, stop) in &segments {
            let (svg_content, w, h) = self.render_sentence(score, lyric, *start, *stop);
            total_width += w;
            if h > max_height {
                max_height = h;
            }
            segment_svgs.push((svg_content, w, h));
        }

        // Build final SVG
        let final_width = total_width + cfg.lane_padding as f64 * 2.0;
        let final_height = max_height
            + cfg.time_padding as f64 * 2.0
            + cfg.meta_size as f64
            + cfg.time_padding as f64 * 2.0;

        let mut svg = String::with_capacity(1024 * 64);
        write!(
            svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{}" height="{}">"#,
            round(final_width),
            round(final_height),
        ).unwrap();

        // Defs
        write!(svg, "<defs>").unwrap();
        write!(svg, "<style>{}</style>", self.style_sheet).unwrap();

        // Gradients
        svg.push_str(r#"<linearGradient id="decoration-gradient" x1="0" y1="1" x2="0" y2="0">"#);
        svg.push_str(r#"<stop offset="0" stop-color="var(--color-start)"/>"#);
        svg.push_str(r#"<stop offset="1" stop-color="var(--color-stop)"/>"#);
        svg.push_str("</linearGradient>");

        svg.push_str(
            r#"<linearGradient id="decoration-critical-gradient" x1="0" y1="1" x2="0" y2="0">"#,
        );
        svg.push_str(r#"<stop offset="0" stop-color="var(--color-start)"/>"#);
        svg.push_str(r#"<stop offset="1" stop-color="var(--color-stop)"/>"#);
        svg.push_str("</linearGradient>");

        // Note symbols
        self.write_note_symbols(&mut svg);
        svg.push_str("</defs>");

        // Background
        write!(
            svg,
            r#"<rect x="0" y="0" width="{}" height="{}" class="background"/>"#,
            round(final_width),
            round(max_height + cfg.time_padding as f64 * 2.0),
        )
        .unwrap();

        // Meta area
        write!(
            svg,
            r#"<rect x="0" y="{}" width="{}" height="{}" class="meta"/>"#,
            round(max_height + cfg.time_padding as f64 * 2.0),
            round(final_width),
            round(cfg.meta_size as f64 + cfg.time_padding as f64 * 2.0),
        )
        .unwrap();

        // Meta line
        write!(
            svg,
            r#"<line x1="0" y1="{y}" x2="{x2}" y2="{y}" class="meta-line"/>"#,
            y = round(max_height + cfg.time_padding as f64 * 2.0),
            x2 = round(final_width),
        )
        .unwrap();

        // Jacket image
        let jacket_url = score
            .meta
            .jacket
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(
                "https://storage.sekai.best/sekai-jp-assets/thumbnail/chara_rip/res009_no021_normal.png",
            );
        write!(
            svg,
            r#"<image href="{}" x="{}" y="{}" width="{}" height="{}"/>"#,
            escape_xml(jacket_url),
            cfg.lane_padding * 2,
            round(max_height + cfg.time_padding as f64 * 3.0),
            cfg.meta_size,
            cfg.meta_size,
        )
        .unwrap();

        // Title
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

        write!(
            svg,
            r#"<text x="{}" y="{}" class="title">{}</text>"#,
            cfg.meta_size + cfg.lane_padding * 4,
            round(cfg.meta_size as f64 + max_height + cfg.time_padding as f64 * 3.0 - 16.0),
            escape_xml(&title),
        )
        .unwrap();

        // Subtitle — match Python's truthiness: difficulty 0 is falsy (eval("0")→0),
        // empty strings are falsy
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
                self.config.generator
            )),
        ]
        .iter()
        .filter_map(|x| x.clone())
        .collect();
        let subtitle = subtitle_parts.join(" ");

        write!(
            svg,
            r#"<text x="{}" y="{}" class="subtitle">{}</text>"#,
            cfg.meta_size + cfg.lane_padding * 4,
            round(cfg.meta_size as f64 / 3.0 + max_height + cfg.time_padding as f64 * 3.0 - 8.0),
            escape_xml(&subtitle),
        )
        .unwrap();

        // Add segments
        let mut x_offset: f64 = 0.0;
        for (svg_content, w, h) in &segment_svgs {
            write!(
                svg,
                r#"<svg x="{}" y="{}" width="{}" height="{}">{}</svg>"#,
                round(x_offset + cfg.lane_padding as f64),
                round(max_height - h + cfg.time_padding as f64),
                round(*w),
                round(*h),
                svg_content,
            )
            .unwrap();
            x_offset += w;
        }

        svg.push_str("</svg>");
        svg
    }

    pub(crate) fn build_skill_covers(&mut self, score: &mut Score) {
        self.build_fever_covers(score);
        self.build_skill_event_covers(score);
    }

    fn build_fever_covers(&mut self, score: &mut Score) {
        let Some((fever_end_time, fever_score)) = self
            .music_meta
            .as_ref()
            .map(|music_meta| (music_meta.fever_end_time, music_meta.fever_score))
        else {
            return;
        };
        let fever_end_bar = score.get_bar_by_time(fever_end_time);
        for event in score
            .events
            .clone()
            .into_iter()
            .filter(|event| event.text.as_deref() == Some("SUPER FEVER!!"))
        {
            self.special_cover_objects.push(CoverObject::Rect {
                bar_from: event.bar,
                css_class: "fever-duration".to_string(),
                bar_to: fever_end_bar,
            });
            self.special_cover_objects.push(CoverObject::Text {
                bar_from: event.bar,
                css_class: "skill-score".to_string(),
                text: format!("multi+{:.2}%", fever_score * 100.0),
            });
        }
    }

    fn build_skill_event_covers(&mut self, score: &mut Score) {
        let events = score.events.clone();
        for (skill_i, event) in events
            .iter()
            .filter(|event| event.text.as_deref() == Some("SKILL"))
            .enumerate()
        {
            let skill_time = score.get_time_f64(event.bar);
            self.special_cover_objects.push(CoverObject::Rect {
                bar_from: score.get_bar_by_time(skill_time - 5.0 / 60.0),
                css_class: "skill-great".to_string(),
                bar_to: score.get_bar_by_time(skill_time + 5.0 + 5.0 / 60.0),
            });
            self.special_cover_objects.push(CoverObject::Rect {
                bar_from: score.get_bar_by_time(skill_time - 2.5 / 60.0),
                css_class: "skill-perfect".to_string(),
                bar_to: score.get_bar_by_time(skill_time + 5.0 + 2.5 / 60.0),
            });
            self.special_cover_objects.push(CoverObject::Rect {
                bar_from: event.bar,
                css_class: "skill-duration".to_string(),
                bar_to: score.get_bar_by_time(skill_time + 5.0),
            });

            if let Some(text) = self.skill_score_text(skill_i) {
                self.special_cover_objects.push(CoverObject::Text {
                    bar_from: event.bar,
                    css_class: "skill-score".to_string(),
                    text,
                });
            }
        }
    }

    fn skill_score_text(&self, skill_idx: usize) -> Option<String> {
        let music_meta = self.music_meta.as_ref()?;
        let solo = music_meta.skill_score_solo.get(skill_idx)?;
        let multi = music_meta.skill_score_multi.get(skill_idx)?;
        let solo = format!("+{:.2}%", solo * 100.0);
        let multi = format!("+{:.2}%", multi * 100.0);
        Some(if solo == multi {
            solo
        } else {
            format!("solo{solo} multi{multi}")
        })
    }

    fn write_note_symbols(&self, svg: &mut String) {
        let cfg = &self.config;
        let note_m_ratio = 1200;

        for note_number in 0..7 {
            // Base symbol
            write!(
                svg,
                r#"<symbol id="notes-{note_number}" viewBox="0 0 112 56">"#,
            )
            .unwrap();
            self.write_note_symbol_body(svg, note_number, -3.0, -3.0, 118.0, 62.0);
            svg.push_str("</symbol>");

            // Middle symbol
            write!(
                svg,
                r#"<symbol id="notes-{note_number}-middle" viewBox="0 0 {} 56">"#,
                112 * note_m_ratio,
            )
            .unwrap();
            self.write_note_symbol_body(
                svg,
                note_number,
                (-(3 + 28) * note_m_ratio) as f64,
                -3.0,
                (118 * note_m_ratio) as f64,
                62.0,
            );
            svg.push_str("</symbol>");

            // Per-lane symbols
            for i in 1..=cfg.n_lanes {
                let note_height = cfg.note_size as f64;
                let note_width = cfg.lane_width as f64 * (i + 1) as f64;
                let note_inner_width = cfg.lane_width as f64 * i as f64;

                let note_l_width = note_height / 56.0 * 32.0;
                let note_r_width = note_l_width;
                let note_m_width = note_inner_width - (note_l_width + note_r_width) / 2.0 - 2.0;
                let note_padding_x =
                    (note_width - note_l_width - note_m_width - note_r_width) / 2.0;

                write!(
                    svg,
                    r#"<symbol id="notes-{note_number}-{i}" viewBox="0 0 {note_width} {note_height}">"#,
                ).unwrap();

                // Left clip path
                write!(
                    svg,
                    r#"<clipPath id="notes-{note_number}-{i}-left"><rect x="0" y="0" width="{note_l_width}" height="{note_height}"/></clipPath>"#,
                ).unwrap();

                // Middle clip path
                write!(
                    svg,
                    r#"<clipPath id="notes-{note_number}-{i}-middle"><rect x="0" y="0" width="{note_m_width}" height="{note_height}"/></clipPath>"#,
                ).unwrap();

                // Right clip path
                write!(
                    svg,
                    r#"<clipPath id="notes-{note_number}-{i}-right"><rect x="{}" y="0" width="{note_r_width}" height="{note_height}"/></clipPath>"#,
                    note_height / 56.0 * 80.0,
                ).unwrap();

                // Left use
                write!(
                    svg,
                    r##"<use href="#notes-{note_number}" x="{}" y="0" width="{}" height="{note_height}" clip-path="url(#notes-{note_number}-{i}-left)"/>"##,
                    note_padding_x,
                    note_height * 2.0,
                ).unwrap();

                // Middle use
                write!(
                    svg,
                    r##"<use href="#notes-{note_number}-middle" x="{}" y="0" width="{}" height="{note_height}" clip-path="url(#notes-{note_number}-{i}-middle)"/>"##,
                    note_padding_x + note_l_width,
                    note_height * note_m_ratio as f64 * 2.0,
                ).unwrap();

                // Right use
                write!(
                    svg,
                    r##"<use href="#notes-{note_number}" x="{}" y="0" width="{}" height="{note_height}" clip-path="url(#notes-{note_number}-{i}-right)"/>"##,
                    note_padding_x + note_l_width + note_m_width + note_r_width - note_height * 2.0,
                    note_height * 2.0,
                ).unwrap();

                svg.push_str("</symbol>");
            }
        }
    }

    fn write_note_symbol_body(
        &self,
        svg: &mut String,
        note_number: i32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) {
        write!(
            svg,
            r#"<image href="{}/notes_{note_number}.{}" x="{}" y="{}" width="{}" height="{}"/>"#,
            self.config.note_host,
            self.config.note_asset_extension,
            format_g(x),
            format_g(y),
            format_g(width),
            format_g(height),
        )
        .unwrap();
    }

    /// Render a single segment (sentence) of the chart
    fn render_sentence(
        &self,
        score: &mut Score,
        lyric: Option<&Lyric>,
        bar_start: i32,
        bar_stop: i32,
    ) -> (String, f64, f64) {
        let cfg = &self.config;
        let bar_start_f = Fraction::from_integer(bar_start as i64);
        let bar_stop_f = Fraction::from_integer(bar_stop as i64);

        let height = cfg.time_height * score.get_time_delta_f64(bar_start_f, bar_stop_f);
        let width = cfg.lane_width as f64 * cfg.n_lanes as f64 + cfg.lane_padding as f64 * 2.0;

        let mut slide_paths = String::new();
        let mut among_images = String::new();
        let mut note_images = String::new();
        let mut flick_images_rev: Vec<String> = Vec::new();
        let mut tick_texts = String::new();
        let mut speed_lines = String::new();

        // Process notes
        let active = score.active_notes.clone();
        let notes_snapshot: Vec<NoteData> = score.notes.clone();

        for (idx_in_active, &note_idx) in active.iter().enumerate() {
            let note = &notes_snapshot[note_idx];
            if !sentence_note_visible(note, &notes_snapshot, bar_start_f, bar_stop_f) {
                continue;
            }

            self.write_sentence_tick(
                score,
                &notes_snapshot,
                &active[idx_in_active..],
                note_idx,
                bar_stop_f,
                &mut tick_texts,
            );

            self.write_sentence_note(
                score,
                &notes_snapshot,
                note_idx,
                bar_stop_f,
                &mut slide_paths,
                &mut among_images,
                &mut note_images,
                &mut flick_images_rev,
            );
        }

        // Build the sentence SVG
        let mut svg = String::with_capacity(1024 * 16);

        // Background
        write!(
            svg,
            r#"<rect x="0" y="0" width="{}" height="{}" class="background"/>"#,
            round(width),
            round(height + cfg.time_padding as f64 * 2.0),
        )
        .unwrap();

        // Lane background
        write!(
            svg,
            r#"<rect x="{}" y="0" width="{}" height="{}" class="lane"/>"#,
            cfg.lane_padding,
            round(cfg.lane_width as f64 * cfg.n_lanes as f64),
            round(height + cfg.time_padding as f64 * 2.0),
        )
        .unwrap();

        self.write_cover_objects(score, bar_start_f, bar_stop_f, &mut svg);

        self.write_lane_lines(height, &mut svg);

        self.write_bar_and_beat_lines(score, bar_start, bar_stop, bar_stop_f, &mut svg);

        let print_events = self.write_event_flags(
            score,
            bar_start,
            bar_stop,
            bar_stop_f,
            &mut svg,
            &mut speed_lines,
        );

        self.write_event_texts(score, &print_events, bar_start_f, bar_stop_f, &mut svg);

        self.write_lyrics(score, lyric, bar_start_f, bar_stop_f, &mut svg);

        // Layer order: slides → notes → amongs → flicks (reversed) → ticks → speed lines
        // Speed lines are drawn last so they appear on top of notes for readability.
        svg.push_str(&slide_paths);
        svg.push_str(&note_images);
        svg.push_str(&among_images);
        for flick in flick_images_rev.iter().rev() {
            svg.push_str(flick);
        }
        svg.push_str(&tick_texts);
        svg.push_str(&speed_lines);

        (svg, width, height + cfg.time_padding as f64 * 2.0)
    }

    fn write_sentence_tick(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        remaining_active: &[NoteIdx],
        note_idx: NoteIdx,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        match arena[note_idx].is_tick(arena) {
            Some(true) => {
                let next_idx = next_tick_note(arena, remaining_active, note_idx);
                self.write_tick_text(score, arena, note_idx, Some(next_idx), bar_stop, out);
            }
            Some(false) => self.write_short_tick_line(score, &arena[note_idx], bar_stop, out),
            None => {}
        }
    }

    fn write_short_tick_line(
        &self,
        score: &mut Score,
        note: &NoteData,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let y = self.config.time_height * score.get_time_delta_f64(note.bar(), bar_stop)
            + self.config.time_padding as f64;
        write!(
            out,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" class="tick-line"/>"#,
            round(self.config.lane_padding as f64 - self.config.tick_2_length as f64),
            round(y),
            round(self.config.lane_padding as f64),
            round(y),
        )
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn write_sentence_note(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        bar_stop: Fraction,
        slide_paths: &mut String,
        among_images: &mut String,
        note_images: &mut String,
        flick_images: &mut Vec<String>,
    ) {
        match &arena[note_idx] {
            NoteData::Tap(..) => {
                self.write_note_image(score, arena, note_idx, bar_stop, note_images);
            }
            NoteData::Directional(..) => {
                self.write_flick_image(score, arena, note_idx, bar_stop, flick_images);
                self.write_note_image(score, arena, note_idx, bar_stop, note_images);
            }
            NoteData::Slide(_, slide) if slide.decoration => self.write_decoration_slide_note(
                score,
                arena,
                note_idx,
                slide,
                bar_stop,
                slide_paths,
                among_images,
                note_images,
                flick_images,
            ),
            NoteData::Slide(_, slide) => self.write_standard_slide_note(
                score,
                arena,
                note_idx,
                slide,
                bar_stop,
                slide_paths,
                among_images,
                note_images,
                flick_images,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_standard_slide_note(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        slide: &crate::notes::slide::Slide,
        bar_stop: Fraction,
        slide_paths: &mut String,
        among_images: &mut String,
        note_images: &mut String,
        flick_images: &mut Vec<String>,
    ) {
        match SlideType::from_i32(arena[note_idx].note_type()) {
            Some(SlideType::Start) => {
                self.write_slide_path(score, arena, note_idx, bar_stop, slide_paths, among_images);
                self.write_note_image(score, arena, note_idx, bar_stop, note_images);
            }
            Some(SlideType::End) => {
                if slide.directional_idx != NO_NOTE {
                    self.write_flick_image(score, arena, note_idx, bar_stop, flick_images);
                }
                self.write_note_image(score, arena, note_idx, bar_stop, note_images);
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_decoration_slide_note(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        slide: &crate::notes::slide::Slide,
        bar_stop: Fraction,
        slide_paths: &mut String,
        among_images: &mut String,
        note_images: &mut String,
        flick_images: &mut Vec<String>,
    ) {
        if matches!(
            SlideType::from_i32(arena[note_idx].note_type()),
            Some(SlideType::Start)
        ) {
            self.write_slide_path(score, arena, note_idx, bar_stop, slide_paths, among_images);
        }
        if slide.tap_idx == NO_NOTE {
            return;
        }
        self.write_note_image(score, arena, slide.tap_idx, bar_stop, note_images);
        if slide.directional_idx != NO_NOTE {
            self.write_flick_image(score, arena, note_idx, bar_stop, flick_images);
        }
    }

    fn write_cover_objects(
        &self,
        score: &mut Score,
        bar_start: Fraction,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        for cover in &self.special_cover_objects {
            match cover {
                CoverObject::Text {
                    bar_from,
                    css_class,
                    text,
                } => self
                    .write_text_cover(score, *bar_from, css_class, text, bar_start, bar_stop, out),
                CoverObject::Rect {
                    bar_from,
                    css_class,
                    bar_to,
                } => self.write_rect_cover(
                    score, *bar_from, *bar_to, css_class, bar_start, bar_stop, out,
                ),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_text_cover(
        &self,
        score: &mut Score,
        bar_from: Fraction,
        css_class: &str,
        text: &str,
        bar_start: Fraction,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        if bar_from < bar_start - Fraction::from_f64(0.2)
            || bar_from >= bar_stop - Fraction::from_f64(0.1)
        {
            return;
        }
        let y = self.config.time_height * score.get_time_delta_f64(bar_from, bar_stop)
            + self.config.time_padding as f64;
        let x = self.config.lane_width as f64 * self.config.n_lanes as f64
            + self.config.lane_padding as f64 * 2.0
            - 3.0;
        write!(
            out,
            r#"<text x="{}" y="{}" transform="rotate(-90, {}, {})" class="{}">{}</text>"#,
            round(x),
            round(y),
            round(x),
            round(y),
            escape_xml(css_class),
            escape_xml(text),
        )
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn write_rect_cover(
        &self,
        score: &mut Score,
        bar_from: Fraction,
        bar_to: Fraction,
        css_class: &str,
        bar_start: Fraction,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let cover_from = bar_from.max(bar_start - Fraction::from_f64(0.2));
        let cover_to = bar_to.min(bar_stop + Fraction::from_f64(0.2));
        if cover_to <= cover_from {
            return;
        }
        let y = self.config.time_height * score.get_time_delta_f64(cover_to, bar_stop)
            + self.config.time_padding as f64;
        let height = self.config.time_height * score.get_time_delta_f64(cover_from, cover_to);
        write!(
            out,
            r#"<rect x="{}" y="{}" width="{}" height="{}" class="{}"/>"#,
            self.config.lane_padding,
            round(y),
            round(self.config.lane_width as f64 * self.config.n_lanes as f64),
            round(height),
            escape_xml(css_class),
        )
        .unwrap();
    }

    fn write_lane_lines(&self, height: f64, out: &mut String) {
        for lane in (0..=self.config.n_lanes).step_by(2) {
            let x = self.config.lane_width as f64 * lane as f64 + self.config.lane_padding as f64;
            write!(
                out,
                r#"<line x1="{}" y1="0" x2="{}" y2="{}" class="lane-line"/>"#,
                round(x),
                round(x),
                round(height + self.config.time_padding as f64 * 2.0),
            )
            .unwrap();
        }
    }

    fn write_bar_and_beat_lines(
        &self,
        score: &mut Score,
        bar_start: i32,
        bar_stop: i32,
        bar_stop_fraction: Fraction,
        out: &mut String,
    ) {
        for bar in bar_start..=bar_stop {
            let bar = Fraction::from_integer(bar as i64);
            self.write_bar_line(score, bar, bar_stop_fraction, out);
            self.write_beat_lines(score, bar, bar_stop_fraction, out);
        }
    }

    fn write_bar_line(
        &self,
        score: &mut Score,
        bar: Fraction,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let y = self.config.time_height * score.get_time_delta_f64(bar, bar_stop)
            + self.config.time_padding as f64;
        let x1 = self.config.lane_padding as f64;
        let x2 = self.config.lane_width as f64 * self.config.n_lanes as f64 + x1;
        write!(
            out,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" class="bar-line"/>"#,
            round(x1),
            round(y),
            round(x2),
            round(y),
        )
        .unwrap();
    }

    fn write_beat_lines(
        &self,
        score: &mut Score,
        bar: Fraction,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let bar_length = score
            .get_event(bar)
            .bar_length
            .unwrap_or(Fraction::from_integer(4));
        let x1 = self.config.lane_padding as f64;
        let x2 = self.config.lane_width as f64 * self.config.n_lanes as f64 + x1;
        for beat_idx in 1..bar_length.to_f64().ceil() as i32 {
            let beat_bar = bar + Fraction::new(beat_idx as i64, 1) / bar_length;
            let y = self.config.time_height * score.get_time_delta_f64(beat_bar, bar_stop)
                + self.config.time_padding as f64;
            write!(
                out,
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" class="beat-line"/>"#,
                round(x1),
                round(y),
                round(x2),
                round(y),
            )
            .unwrap();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_event_flags(
        &self,
        score: &mut Score,
        bar_start: i32,
        bar_stop: i32,
        bar_stop_fraction: Fraction,
        out: &mut String,
        speed_lines: &mut String,
    ) -> Vec<Event> {
        let mut print_events = Vec::new();
        for event in sentence_events(score, bar_start, bar_stop) {
            if let Some(speed) = event.speed {
                self.write_speed_event(score, &event, speed, bar_stop_fraction, speed_lines);
                continue;
            }
            merge_print_event(&mut print_events, &event);
            self.write_event_flag(score, &event, bar_stop_fraction, out);
        }
        print_events
    }

    fn write_speed_event(
        &self,
        score: &mut Score,
        event: &Event,
        speed: f64,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let y = self.config.time_height * score.get_time_delta_f64(event.bar, bar_stop)
            + self.config.time_padding as f64;
        let x1 = self.config.lane_padding as f64;
        let x2 = self.config.lane_width as f64 * self.config.n_lanes as f64 + x1;
        write!(
            out,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" class="speed-line"/>"#,
            round(x1),
            round(y),
            round(x2),
            round(y),
        )
        .unwrap();
        write!(
            out,
            r#"<text x="{}" y="{}" class="speed-text">{}x</text>"#,
            round(x2 - 2.0),
            round(y - 2.0),
            format_g(speed),
        )
        .unwrap();
    }

    fn write_event_flag(
        &self,
        score: &mut Score,
        event: &Event,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let y = self.config.time_height * score.get_time_delta_f64(event.bar, bar_stop)
            + self.config.time_padding as f64;
        write!(
            out,
            r#"<line x1="0" y1="{}" x2="{}" y2="{}" class="{}"/>"#,
            round(y),
            round(self.config.lane_padding as f64),
            round(y),
            if event_is_special(event) {
                "event-flag"
            } else {
                "bar-count-flag"
            },
        )
        .unwrap();
    }

    fn write_event_texts(
        &self,
        score: &mut Score,
        events: &[Event],
        bar_start: Fraction,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        for event in events {
            if !bar_in_sentence(event.bar, bar_start, bar_stop) {
                continue;
            }
            let text = event_label(event);
            if text.is_empty() {
                continue;
            }
            let y = self.config.time_height * score.get_time_delta_f64(event.bar, bar_stop)
                + self.config.time_padding as f64;
            write!(
                out,
                r#"<text x="{}" y="{}" transform="rotate(-90, {}, {})" class="{}">{}</text>"#,
                round(self.config.lane_padding as f64 + 8.0),
                round(y - self.config.lane_width as f64 * 1.5),
                round(self.config.lane_padding as f64),
                round(y),
                if event_is_special(event) {
                    "event-text"
                } else {
                    "bar-count-text"
                },
                escape_xml(&text),
            )
            .unwrap();
        }
    }

    fn write_lyrics(
        &self,
        score: &mut Score,
        lyric: Option<&Lyric>,
        bar_start: Fraction,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let Some(lyric) = lyric else { return };
        for word in lyric
            .words
            .iter()
            .filter(|word| bar_in_sentence(word.bar, bar_start, bar_stop))
        {
            let y = self.config.time_height * score.get_time_delta_f64(word.bar, bar_stop)
                + self.config.time_padding as f64;
            let x = self.config.lane_width as f64 * self.config.n_lanes as f64
                + self.config.lane_padding as f64;
            write!(
                out,
                r#"<text x="{}" y="{}" transform="rotate(-90, {}, {})" class="lyric-text">{}</text>"#,
                round(x),
                round(y + 16.0),
                round(x),
                round(y),
                escape_xml(&word.text),
            )
            .unwrap();
        }
    }

    fn write_slide_path(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        start_idx: NoteIdx,
        bar_stop: Fraction,
        slide_paths: &mut String,
        among_images: &mut String,
    ) {
        let (lefts, rights) =
            self.collect_slide_edges(score, arena, start_idx, bar_stop, among_images);
        if lefts.is_empty() {
            return;
        }
        let class_name = slide_path_class(&arena[start_idx], arena);
        let d = slide_path_data(&lefts, &rights);
        write!(slide_paths, r#"<path d="{d}" class="{class_name}"/>"#).unwrap();
    }

    fn collect_slide_edges(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        start_idx: NoteIdx,
        bar_stop: Fraction,
        among_images: &mut String,
    ) -> (Vec<BezierPoints>, Vec<BezierPoints>) {
        let mut lefts = Vec::new();
        let mut rights = Vec::new();
        let mut current_idx = start_idx;
        loop {
            if matches!(
                SlideType::from_i32(arena[current_idx].note_type()),
                Some(SlideType::End)
            ) {
                break;
            }
            let Some(slide) = arena[current_idx].as_slide() else {
                break;
            };
            if slide.next_idx == NO_NOTE {
                break;
            }
            let (next_idx, amongs) = next_slide_path_node(arena, slide.next_idx);
            let (left, right) =
                self.get_bezier_coordinates(score, arena, current_idx, next_idx, bar_stop);
            self.write_slide_amongs(score, arena, &amongs, bar_stop, &left, &right, among_images);
            lefts.push(left);
            rights.push(right);
            current_idx = next_idx;
        }
        (lefts, rights)
    }

    #[allow(clippy::too_many_arguments)]
    fn write_slide_amongs(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        amongs: &[NoteIdx],
        bar_stop: Fraction,
        left: &BezierPoints,
        right: &BezierPoints,
        out: &mut String,
    ) {
        let size = self.config.lane_width as f64;
        for &among_idx in amongs {
            let y = self.config.time_height
                * score.get_time_delta_f64(arena[among_idx].bar(), bar_stop)
                + self.config.time_padding as f64;
            let x = (binary_solution_for_x(y, left) + binary_solution_for_x(y, right)) / 2.0;
            self.write_long_among_image(
                arena[among_idx].is_critical(arena),
                x - size / 2.0,
                y - size / 2.0,
                size,
                size,
                out,
            );
        }
    }

    fn get_bezier_coordinates(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        idx0: NoteIdx,
        idx1: NoteIdx,
        bar_stop: Fraction,
    ) -> (BezierPoints, BezierPoints) {
        let cfg = &self.config;
        let slide_0 = &arena[idx0];
        let slide_1 = &arena[idx1];

        let y_0 = cfg.time_height * score.get_time_delta_f64(slide_0.bar(), bar_stop)
            + cfg.time_padding as f64;
        let y_1 = cfg.time_height * score.get_time_delta_f64(slide_1.bar(), bar_stop)
            + cfg.time_padding as f64;

        let curve_direction = slide_curve_direction(slide_0, arena);
        let ease_in = matches!(curve_direction, Some(DirectionalType::Down));
        let ease_out = matches!(
            curve_direction,
            Some(DirectionalType::LowerLeft | DirectionalType::LowerRight)
        );

        let is_decoration = slide_0.as_slide().map(|s| s.decoration).unwrap_or(false);
        let spp = if is_decoration {
            0.0
        } else {
            cfg.slide_path_padding
        };

        let l0_x =
            cfg.lane_width as f64 * (slide_0.lane() - 2) as f64 + cfg.lane_padding as f64 - spp;
        let l1_x =
            cfg.lane_width as f64 * (slide_1.lane() - 2) as f64 + cfg.lane_padding as f64 - spp;
        let r0_x = cfg.lane_width as f64 * (slide_0.lane() - 2 + slide_0.width()) as f64
            + cfg.lane_padding as f64
            + spp;
        let r1_x = cfg.lane_width as f64 * (slide_1.lane() - 2 + slide_1.width()) as f64
            + cfg.lane_padding as f64
            + spp;

        let mid_y = (y_0 + y_1) / 2.0;

        let left = [
            (l0_x, y_0),
            (l0_x, if ease_in { mid_y } else { y_0 }),
            (l1_x, if ease_out { mid_y } else { y_1 }),
            (l1_x, y_1),
        ];

        let right = [
            (r0_x, y_0),
            (r0_x, if ease_in { mid_y } else { y_0 }),
            (r1_x, if ease_out { mid_y } else { y_1 }),
            (r1_x, y_1),
        ];

        (left, right)
    }

    fn write_note_image(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let cfg = &self.config;
        let note = &arena[note_idx];

        let y = cfg.time_height * score.get_time_delta_f64(note.bar(), bar_stop)
            + cfg.time_padding as f64;
        let x = cfg.lane_width as f64 * (note.lane() as f64 - 2.5) + cfg.lane_padding as f64;
        let w = cfg.lane_width as f64 * (note.width() + 1) as f64;
        let h = cfg.lane_width as f64 / 64.0 * 56.0 * 2.0;

        if note.is_none(arena) {
            return;
        }

        if note.is_trend(arena) {
            self.write_friction_among_image(score, arena, note_idx, bar_stop, out);
        }
        let note_number = note_image_number(note, arena);

        write!(
            out,
            r##"<use href="#notes-{}-{}" x="{}" y="{}" width="{}" height="{}"/>"##,
            note_number,
            note.width(),
            round(x),
            round(y - h / 2.0),
            round(w),
            round(h),
        )
        .unwrap();
    }

    fn write_friction_among_image(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let cfg = &self.config;
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

        self.write_among_image(kind, x - w / 2.0, y - h / 2.0, w, h, out);
    }

    fn write_long_among_image(
        &self,
        is_critical: bool,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        out: &mut String,
    ) {
        let kind = if is_critical {
            AmongKind::LongAmongCritical
        } else {
            AmongKind::LongAmong
        };
        self.write_among_image(kind, x, y, width, height, out);
    }

    fn write_among_image(
        &self,
        kind: AmongKind,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        out: &mut String,
    ) {
        let filename = match kind {
            AmongKind::LongAmong => "notes_long_among",
            AmongKind::LongAmongCritical => "notes_long_among_crtcl",
            AmongKind::FrictionLong => "notes_friction_among_long",
            AmongKind::FrictionFlick => "notes_friction_among_flick",
            AmongKind::FrictionCritical => "notes_friction_among_crtcl",
        };
        write!(
            out,
            r#"<image href="{}/{}.{}" x="{}" y="{}" width="{}" height="{}"/>"#,
            self.config.note_host,
            filename,
            self.config.note_asset_extension,
            round(x),
            round(y),
            round(width),
            round(height),
        )
        .unwrap();
    }

    fn write_flick_image(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        bar_stop: Fraction,
        out: &mut Vec<String>,
    ) {
        let cfg = &self.config;
        let note = &arena[note_idx];

        let y = cfg.time_height * score.get_time_delta_f64(note.bar(), bar_stop)
            + cfg.time_padding as f64;

        if note.is_none(arena) {
            return;
        }

        let flick_type = match note_directional_type(note, arena) {
            Some(DirectionalType::UpperLeft) => Some(DirectionalType::UpperLeft),
            Some(DirectionalType::UpperRight) => Some(DirectionalType::UpperRight),
            Some(DirectionalType::Up) => Some(DirectionalType::Up),
            Some(_) => None,
            None => Some(DirectionalType::Up),
        };

        let Some(flick_type) = flick_type else { return };

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

        let mut img = String::new();
        let img_x = x - w / 2.0 + bias;
        let img_y = y + cfg.note_size as f64 / 4.0 - h;

        let flip_right = matches!(flick_type, DirectionalType::UpperRight);
        write!(
            img,
            r#"<image href="{}/notes_flick_arrow{}_{:02}{}.{}" x="{}" y="{}" width="{}" height="{}""#,
            self.config.note_host,
            if is_crit { "_crtcl" } else { "" },
            width,
            if is_diagonal { "_diagonal" } else { "" },
            self.config.note_asset_extension,
            round(img_x),
            round(img_y),
            round(w),
            round(h),
        )
        .unwrap();

        if flip_right {
            write!(
                img,
                r#" transform-origin="{} 0" transform="scale(-1, 1)""#,
                round(x + bias),
            )
            .unwrap();
        }

        img.push_str("/>");
        out.push(img);
    }

    fn write_tick_text(
        &self,
        score: &mut Score,
        arena: &[NoteData],
        note_idx: NoteIdx,
        next_idx: Option<NoteIdx>,
        bar_stop: Fraction,
        out: &mut String,
    ) {
        let cfg = &self.config;
        let note = &arena[note_idx];
        let y = cfg.time_height * score.get_time_delta_f64(note.bar(), bar_stop)
            + cfg.time_padding as f64;

        let next_idx = match next_idx {
            Some(idx) => idx,
            None => {
                // Just draw a short tick line
                write!(
                    out,
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}" class="tick-line"/>"#,
                    round(cfg.lane_padding as f64 - cfg.tick_2_length as f64),
                    round(y),
                    round(cfg.lane_padding as f64),
                    round(y),
                )
                .unwrap();
                return;
            }
        };

        let next = &arena[next_idx];
        if next.bar() == note.bar() || (next.bar() - note.bar()).to_f64() > 1.0 {
            // Use distance to next bar
            let interval_frac = Fraction::from_integer(note.bar().floor() + 1) - note.bar();
            self.write_tick_with_interval(score, y, interval_frac, note.bar(), out);
        } else if (next.bar() - note.bar()).to_f64() > 0.5
            && next.bar().floor() != note.bar().floor()
        {
            let interval_frac = Fraction::from_integer(note.bar().floor() + 1) - note.bar();
            self.write_tick_with_interval(score, y, interval_frac, note.bar(), out);
        } else {
            let interval_frac = next.bar() - note.bar();
            self.write_tick_with_interval(score, y, interval_frac, note.bar(), out);
        }
    }

    fn write_tick_with_interval(
        &self,
        score: &mut Score,
        y: f64,
        interval: Fraction,
        bar: Fraction,
        out: &mut String,
    ) {
        let cfg = &self.config;
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

        write!(
            out,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" class="tick-line"/>"#,
            round(cfg.lane_padding as f64 - cfg.tick_length as f64),
            round(y),
            round(cfg.lane_padding as f64),
            round(y),
        )
        .unwrap();

        write!(
            out,
            r#"<text x="{}" y="{}" class="tick-text">{}</text>"#,
            round(cfg.lane_padding as f64 - 4.0),
            round(y - 2.0),
            escape_xml(&text),
        )
        .unwrap();
    }
}

fn sentence_note_visible(
    note: &NoteData,
    arena: &[NoteData],
    bar_start: Fraction,
    bar_stop: Fraction,
) -> bool {
    if !note.is_slide() {
        return bar_in_sentence(note.bar(), bar_start, bar_stop);
    }
    note.as_slide()
        .is_some_and(|slide| slide_chain_visible(slide.head_idx, arena, bar_start, bar_stop))
}

fn slide_chain_visible(
    head_idx: NoteIdx,
    arena: &[NoteData],
    bar_start: Fraction,
    bar_stop: Fraction,
) -> bool {
    if head_idx == NO_NOTE {
        return false;
    }
    let mut current_idx = next_visible_slide_path_node(head_idx, arena);
    let mut found_before = false;
    while let Some(note_idx) = current_idx {
        let bar = arena[note_idx].bar();
        if bar_in_sentence(bar, bar_start, bar_stop) {
            return true;
        }
        if bar < bar_start - Fraction::from_integer(1) {
            found_before = true;
        } else if found_before && bar_stop + Fraction::from_integer(1) < bar {
            return true;
        }
        current_idx = following_visible_slide_path_node(note_idx, arena);
    }
    false
}

fn next_visible_slide_path_node(start_idx: NoteIdx, arena: &[NoteData]) -> Option<NoteIdx> {
    let mut note_idx = start_idx;
    loop {
        let note = &arena[note_idx];
        let slide = note.as_slide()?;
        if slide.is_path(note.note_type()) {
            return Some(note_idx);
        }
        if slide.next_idx == NO_NOTE {
            return None;
        }
        note_idx = slide.next_idx;
    }
}

fn following_visible_slide_path_node(note_idx: NoteIdx, arena: &[NoteData]) -> Option<NoteIdx> {
    let next_idx = arena[note_idx].as_slide()?.next_idx;
    (next_idx != NO_NOTE)
        .then(|| next_visible_slide_path_node(next_idx, arena))
        .flatten()
}

fn next_tick_note(arena: &[NoteData], active: &[NoteIdx], note_idx: NoteIdx) -> NoteIdx {
    active
        .iter()
        .copied()
        .find(|&candidate_idx| {
            arena[candidate_idx].is_tick(arena) == Some(true)
                && arena[candidate_idx].bar() > arena[note_idx].bar()
        })
        .unwrap_or(note_idx)
}

fn sentence_events(score: &Score, bar_start: i32, bar_stop: i32) -> Vec<Event> {
    let mut events = (bar_start..=bar_stop)
        .map(|bar| Event::new(Fraction::from_integer(bar as i64)))
        .collect::<Vec<_>>();
    events.extend(score.events.clone());
    events.sort_by(|left, right| {
        left.bar
            .partial_cmp(&right.bar)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    events
}

fn merge_print_event(events: &mut Vec<Event>, event: &Event) {
    let Some(last) = events.last_mut() else {
        events.push(event.clone());
        return;
    };
    if (event.bar - last.bar).to_f64() <= 1.0 / 16.0 {
        last.merge_from(event);
    } else {
        events.push(event.clone());
    }
}

fn event_is_special(event: &Event) -> bool {
    event.bpm.is_some()
        || event.bar_length.is_some()
        || event.speed.is_some()
        || event.section.is_some()
        || event.text.is_some()
}

fn event_label(event: &Event) -> String {
    let mut parts = Vec::new();
    if event.bar.trunc() == *event.bar.numer() && *event.bar.denom() == 1 {
        parts.push(format!("#{}", format_g(event.bar.to_f64())));
    }
    if let Some(bpm) = event.bpm {
        parts.push(format!("{} BPM", format_g(bpm.to_f64())));
    }
    if let Some(bar_length) = event.bar_length {
        parts.push(format!("{}/4", format_g(bar_length.to_f64())));
    }
    if let Some(section) = &event.section {
        parts.push(section.clone());
    }
    if let Some(text) = &event.text {
        parts.push(text.clone());
    }
    parts.join(", ")
}

fn bar_in_sentence(bar: Fraction, bar_start: Fraction, bar_stop: Fraction) -> bool {
    bar_start - Fraction::from_integer(1) <= bar && bar < bar_stop + Fraction::from_integer(1)
}

fn slide_curve_direction(note: &NoteData, arena: &[NoteData]) -> Option<DirectionalType> {
    let directional_idx = note.as_slide()?.directional_idx;
    (directional_idx != NO_NOTE)
        .then(|| DirectionalType::from_i32(arena[directional_idx].note_type()))
        .flatten()
}

fn note_directional_type(note: &NoteData, arena: &[NoteData]) -> Option<DirectionalType> {
    if note.is_directional() {
        return DirectionalType::from_i32(note.note_type());
    }
    slide_curve_direction(note, arena)
}

fn note_image_number(note: &NoteData, arena: &[NoteData]) -> i32 {
    if note.is_trend(arena) {
        return if note.is_critical(arena) {
            5
        } else if note.is_directional() {
            6
        } else {
            4
        };
    }
    if note.is_critical(arena) {
        return 0;
    }
    if note.is_directional() {
        return 3;
    }
    if !note.is_slide() {
        return 2;
    }
    let is_directional_end = matches!(SlideType::from_i32(note.note_type()), Some(SlideType::End))
        && note
            .as_slide()
            .is_some_and(|slide| slide.directional_idx != NO_NOTE);
    if is_directional_end { 3 } else { 1 }
}

fn next_slide_path_node(arena: &[NoteData], start_idx: NoteIdx) -> (NoteIdx, Vec<NoteIdx>) {
    let mut amongs = Vec::new();
    let mut next_idx = start_idx;
    loop {
        let note_type = arena[next_idx].note_type();
        if matches!(SlideType::from_i32(note_type), Some(SlideType::Relay)) {
            amongs.push(next_idx);
        }
        let Some(slide) = arena[next_idx].as_slide() else {
            break;
        };
        if slide.is_path(note_type) || slide.next_idx == NO_NOTE {
            break;
        }
        next_idx = slide.next_idx;
    }
    (next_idx, amongs)
}

fn slide_path_class(note: &NoteData, arena: &[NoteData]) -> &'static str {
    let critical = note.is_critical(arena);
    match (
        note.as_slide().is_some_and(|slide| slide.decoration),
        critical,
    ) {
        (true, true) => "decoration-critical",
        (true, false) => "decoration",
        (false, true) => "slide-critical",
        (false, false) => "slide",
    }
}

fn slide_path_data(lefts: &[BezierPoints], rights: &[BezierPoints]) -> String {
    let mut path = String::new();
    for (index, left) in lefts.iter().enumerate() {
        if index == 0 {
            write!(path, "M{},{}", round(left[0].0), round(left[0].1)).unwrap();
        }
        write!(
            path,
            "C{},{},{},{},{},{}",
            round(left[1].0),
            round(left[1].1),
            round(left[2].0),
            round(left[2].1),
            round(left[3].0),
            round(left[3].1),
        )
        .unwrap();
    }
    for (index, right) in rights.iter().rev().enumerate() {
        if index == 0 {
            write!(path, "L{},{}", round(right[3].0), round(right[3].1)).unwrap();
        }
        write!(
            path,
            "C{},{},{},{},{},{}",
            round(right[2].0),
            round(right[2].1),
            round(right[1].0),
            round(right[1].1),
            round(right[0].0),
            round(right[0].1),
        )
        .unwrap();
    }
    path.push('z');
    path
}

/// Binary search to find x-coordinate on a cubic Bézier curve at a given y
fn binary_solution_for_x(y: f64, curve: &[(f64, f64); 4]) -> f64 {
    binary_solution_for_x_inner(y, curve, 0.0, 1.0, 0.1, 100)
}

fn binary_solution_for_x_inner(
    y: f64,
    curve: &[(f64, f64); 4],
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

fn round(v: f64) -> i64 {
    // Match Python's `round()`: banker's rounding (round half to even).
    // f64::round() rounds half away from zero, which differs at exact *.5.
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

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn normalize_note_asset_extension(extension: String) -> String {
    extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
}

/// Format a float like Python's %g (6 significant digits, trailing zeros removed,
/// scientific notation for very small/large values)
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
