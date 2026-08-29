use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::fraction::Fraction;
use crate::line::{
    BpmDefinition, BpmReference, Line, ParsedItem, SpeedControl, SpeedDefinition, TicksPerBeat,
};
use crate::meta::Meta;
use crate::notes::event::Event;
use crate::notes::slide::SlideType;
use crate::notes::{NO_NOTE, NoteData, NoteIdx};
use crate::score_json::{ScoreJsonError, parse_score_json};

/// The main score container. Holds all parsed notes in an arena
/// and events in a sorted list.
#[derive(Clone)]
pub struct Score {
    pub meta: Meta,
    /// Arena of all notes; indices are stable after init
    pub notes: Vec<NoteData>,
    /// Indices into `notes` that are "active" (not deleted during linking)
    pub active_notes: Vec<NoteIdx>,
    pub events: Vec<Event>,
    /// Cached timed events (time_fraction, merged_event) — uses Fraction for exact arithmetic
    pub timed_events_cache: Option<Vec<(Fraction, Event)>>,
    /// Cached exact elapsed time for queried bar positions.
    pub time_cache: HashMap<Fraction, Fraction>,
    /// Cached limited-denominator elapsed time for rendering coordinates.
    pub time_f64_cache: HashMap<Fraction, f64>,
}

impl Score {
    pub fn new() -> Self {
        Score {
            meta: Meta::new(),
            notes: Vec::new(),
            active_notes: Vec::new(),
            events: Vec::new(),
            timed_events_cache: None,
            time_cache: HashMap::new(),
            time_f64_cache: HashMap::new(),
        }
    }

    /// Open and parse a score file. `.json` files and JSON-looking content are parsed
    /// as Project SEKAI custom chart JSON; all other files are parsed as `.sus`.
    pub fn open(path: &str) -> io::Result<Score> {
        let content = fs::read_to_string(path)?;
        if path_looks_json(path) || content_looks_json(&content) {
            return Self::parse_json(&content).map_err(invalid_json_data);
        }

        Ok(Self::parse(&content))
    }

    /// Open and parse a .sus file
    pub fn open_sus(path: &str) -> io::Result<Score> {
        let content = fs::read_to_string(path)?;
        Ok(Self::parse(&content))
    }

    /// Open and parse Project SEKAI custom chart JSON
    pub fn open_json(path: &str) -> io::Result<Score> {
        let content = fs::read_to_string(path)?;
        Self::parse_json(&content).map_err(invalid_json_data)
    }

    /// Parse from .sus string content
    pub fn parse(content: &str) -> Score {
        let mut score = Score::new();
        let lines: Vec<Line> = content.lines().map(Line::new).collect();
        score.init_by_lines(&lines);
        score
    }

    /// Parse from Project SEKAI custom chart JSON string content
    pub fn parse_json(content: &str) -> Result<Score, ScoreJsonError> {
        parse_score_json(content)
    }

    /// Parse score string content, auto-detecting JSON-looking content.
    pub fn parse_auto(content: &str) -> Result<Score, ScoreJsonError> {
        if content_looks_json(content) {
            Self::parse_json(content)
        } else {
            Ok(Self::parse(content))
        }
    }

    fn init_by_lines(&mut self, lines: &[Line]) {
        self.meta = Meta::new();
        self.notes.clear();
        self.events.clear();
        self.timed_events_cache = None;

        let mut bpm_definitions: HashMap<i32, Fraction> = HashMap::new();
        let mut _speed_definitions: HashMap<i32, SpeedDefinition> = HashMap::new();
        let mut _speed_control = SpeedControl { id: None };
        let mut ticks_per_beat = TicksPerBeat::default();

        for line in lines {
            for item in line.parse() {
                match item {
                    ParsedItem::Meta(m) => {
                        self.meta = self.meta.merge(&m);
                    }
                    ParsedItem::TicksPerBeat(tpb) => {
                        ticks_per_beat = tpb;
                    }
                    ParsedItem::SpeedControl(sc) => {
                        _speed_control = sc;
                    }
                    ParsedItem::SpeedDefinition(sd) => {
                        let id = sd.id;
                        for item in &sd.items {
                            let bar = Fraction::from_integer(item.bar as i64)
                                + Fraction::new(item.tick as i64, ticks_per_beat.0 as i64 * 4);
                            self.events.push(Event::new(bar).with_speed(item.speed));
                        }
                        _speed_definitions.insert(id, sd);
                    }
                    ParsedItem::Event(e) => {
                        self.events.push(e);
                    }
                    ParsedItem::BpmDefinition(BpmDefinition { id, bpm }) => {
                        bpm_definitions.insert(id, bpm);
                    }
                    ParsedItem::BpmReference(BpmReference { bar, id }) => {
                        if let Some(&bpm) = bpm_definitions.get(&id) {
                            self.events.push(Event::new(bar).with_bpm(bpm));
                        }
                    }
                    ParsedItem::Note(note) => {
                        self.notes.push(note);
                    }
                }
            }
        }

        self.init_notes();
        self.init_events();
    }

    /// Multi-pass note linking algorithm matching Python's _init_notes
    pub fn init_notes(&mut self) {
        self.notes.sort_by(|a, b| {
            a.bar()
                .partial_cmp(&b.bar())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let n = self.notes.len();
        let mut note_deleted = vec![false; n];
        let mut note_indexes: HashMap<Fraction, Vec<usize>> = HashMap::new();
        self.index_valid_notes(&mut note_deleted, &mut note_indexes);
        self.link_directional_attachments(&mut note_deleted, &note_indexes);
        self.link_slides(&mut note_deleted, &note_indexes);
        self.active_notes = (0..n).filter(|&i| !note_deleted[i]).collect();
    }

    fn index_valid_notes(
        &mut self,
        note_deleted: &mut [bool],
        note_indexes: &mut HashMap<Fraction, Vec<usize>>,
    ) {
        for (note_idx, is_deleted) in note_deleted.iter_mut().enumerate() {
            let lane = self.notes[note_idx].lane();
            if (0..12).contains(&(lane - 2)) {
                note_indexes
                    .entry(self.notes[note_idx].bar())
                    .or_default()
                    .push(note_idx);
                continue;
            }
            *is_deleted = true;
            self.events.push(invalid_note_event(&self.notes[note_idx]));
        }
    }

    fn link_directional_attachments(
        &mut self,
        note_deleted: &mut [bool],
        note_indexes: &HashMap<Fraction, Vec<usize>>,
    ) {
        for note_idx in 0..self.notes.len() {
            if note_deleted[note_idx] || !self.notes[note_idx].is_directional() {
                continue;
            }
            let indexes = note_indexes
                .get(&self.notes[note_idx].bar())
                .cloned()
                .unwrap_or_default();
            self.link_directional_taps(note_idx, &indexes, note_deleted);
        }
    }

    fn link_directional_taps(
        &mut self,
        directional_idx: usize,
        indexes: &[usize],
        note_deleted: &mut [bool],
    ) {
        for &tap_idx in indexes {
            if note_deleted[tap_idx] || !self.notes[tap_idx].is_tap() {
                continue;
            }
            if !notes_share_slot(&self.notes[directional_idx], &self.notes[tap_idx]) {
                continue;
            }
            note_deleted[tap_idx] = true;
            if let Some(directional) = self.notes[directional_idx].as_directional_mut() {
                directional.tap_idx = tap_idx;
            }
        }
    }

    fn link_slides(
        &mut self,
        note_deleted: &mut [bool],
        note_indexes: &HashMap<Fraction, Vec<usize>>,
    ) {
        for slide_idx in 0..self.notes.len() {
            if note_deleted[slide_idx] || !self.notes[slide_idx].is_slide() {
                continue;
            }
            self.initialize_slide_head(slide_idx);
            let indexes = note_indexes
                .get(&self.notes[slide_idx].bar())
                .cloned()
                .unwrap_or_default();
            self.link_slide_taps(slide_idx, &indexes, note_deleted);
            self.link_slide_directionals(slide_idx, &indexes, note_deleted);
            self.link_next_slide(slide_idx, note_deleted);
        }
    }

    fn initialize_slide_head(&mut self, slide_idx: usize) {
        if let Some(slide) = self.notes[slide_idx].as_slide_mut()
            && slide.head_idx == NO_NOTE
        {
            slide.head_idx = slide_idx;
        }
    }

    fn link_slide_taps(&mut self, slide_idx: usize, indexes: &[usize], note_deleted: &mut [bool]) {
        for &tap_idx in indexes {
            if note_deleted[tap_idx] || !self.notes[tap_idx].is_tap() {
                continue;
            }
            if !notes_share_slot(&self.notes[slide_idx], &self.notes[tap_idx]) {
                continue;
            }
            note_deleted[tap_idx] = true;
            if let Some(slide) = self.notes[slide_idx].as_slide_mut() {
                slide.tap_idx = tap_idx;
            }
        }
    }

    fn link_slide_directionals(
        &mut self,
        slide_idx: usize,
        indexes: &[usize],
        note_deleted: &mut [bool],
    ) {
        for &directional_idx in indexes {
            if note_deleted[directional_idx] || !self.notes[directional_idx].is_directional() {
                continue;
            }
            if !notes_share_slot(&self.notes[slide_idx], &self.notes[directional_idx]) {
                continue;
            }
            note_deleted[directional_idx] = true;
            let tap_idx = self.notes[directional_idx]
                .as_directional()
                .map(|directional| directional.tap_idx)
                .unwrap_or(NO_NOTE);
            if let Some(slide) = self.notes[slide_idx].as_slide_mut() {
                slide.directional_idx = directional_idx;
                if tap_idx != NO_NOTE {
                    slide.tap_idx = tap_idx;
                }
            }
        }
    }

    fn link_next_slide(&mut self, slide_idx: usize, note_deleted: &[bool]) {
        let Some(slide) = self.notes[slide_idx].as_slide() else {
            return;
        };
        if matches!(
            SlideType::from_i32(self.notes[slide_idx].note_type()),
            Some(SlideType::End)
        ) {
            return;
        }
        let (channel, decoration, head_idx) = (slide.channel, slide.decoration, slide.head_idx);
        let Some(next_idx) = self.find_next_slide(slide_idx, channel, decoration, note_deleted)
        else {
            return;
        };
        if let Some(slide) = self.notes[slide_idx].as_slide_mut() {
            slide.next_idx = next_idx;
        }
        if let Some(next_slide) = self.notes[next_idx].as_slide_mut() {
            next_slide.head_idx = head_idx;
        }
    }

    fn find_next_slide(
        &self,
        slide_idx: usize,
        channel: i32,
        decoration: bool,
        note_deleted: &[bool],
    ) -> Option<usize> {
        (slide_idx + 1..self.notes.len()).find(|&next_idx| {
            !note_deleted[next_idx]
                && self.notes[next_idx]
                    .as_slide()
                    .is_some_and(|slide| slide.channel == channel && slide.decoration == decoration)
        })
    }

    /// Dedup fully-identical consecutive events (matches Python's `_init_events`,
    /// which uses dataclass-generated `__eq__` comparing all fields).
    /// Events at the same bar with DIFFERENT fields are kept as separate entries.
    pub fn init_events(&mut self) {
        self.events.sort_by(|a, b| {
            a.bar
                .partial_cmp(&b.bar)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut merged: Vec<Event> = Vec::new();
        for event in &self.events {
            if let Some(last) = merged.last_mut()
                && events_equal(last, event)
            {
                continue;
            }
            merged.push(event.clone());
        }
        self.events = merged;
        self.timed_events_cache = None;
        self.time_cache.clear();
        self.time_f64_cache.clear();
    }

    /// Compute timed events: (elapsed_time_fraction, merged_event) list
    /// Uses Fraction for exact arithmetic matching Python's Fraction accumulation
    pub fn timed_events(&mut self) -> &[(Fraction, Event)] {
        if let Some(ref cached) = self.timed_events_cache {
            return cached;
        }

        let mut timed: Vec<(Fraction, Event)> = Vec::new();
        let mut t = Fraction::zero();
        let mut e = Event::new(Fraction::zero());
        e.bpm = Some(Fraction::from_integer(120));
        e.bar_length = Some(Fraction::from_integer(4));
        e.sentence_length = Some(4);

        for event in &self.events {
            let bpm = e.bpm.unwrap_or(Fraction::from_integer(120));
            let bar_length = e.bar_length.unwrap_or(Fraction::from_integer(4));
            let delta_bar = event.bar - e.bar;
            t = (t + delta_bar * bar_length * Fraction::from_integer(60) / bpm)
                .limit_denominator(1_000_000_000);
            e = e.merge(event);
            timed.push((t, e.clone()));
        }

        if timed.is_empty() {
            timed.push((Fraction::zero(), e));
        }

        self.timed_events_cache.insert(timed)
    }

    /// Get time and event at a given bar position (binary search).
    /// Matches Python's `bisect.bisect(...) - 1`: picks the LAST entry whose
    /// bar <= target, so duplicate-bar events resolve to the most-merged state.
    pub fn get_timed_event(&mut self, bar: Fraction) -> (Fraction, Event) {
        let timed = self.timed_events();
        let idx = Self::timed_event_index(timed, bar);
        let (ref t, ref e) = timed[idx];
        (Self::time_from_timed_event(*t, e, bar), e.clone())
    }

    pub fn get_time(&mut self, bar: Fraction) -> Fraction {
        if let Some(time) = self.time_cache.get(&bar) {
            return *time;
        }
        let time = {
            let timed = self.timed_events();
            Self::time_at(timed, bar)
        };
        self.time_cache.insert(bar, time);
        time
    }

    pub fn get_event(&mut self, bar: Fraction) -> Event {
        self.get_timed_event(bar).1
    }

    pub fn get_time_delta(&mut self, bar_from: Fraction, bar_to: Fraction) -> Fraction {
        self.get_time(bar_to) - self.get_time(bar_from)
    }

    pub fn get_time_f64(&mut self, bar: Fraction) -> f64 {
        self.get_time(bar).to_f64()
    }

    pub fn get_time_delta_f64(&mut self, bar_from: Fraction, bar_to: Fraction) -> f64 {
        // Limit each operand's denominator before subtraction to keep
        // Ratio<i64> arithmetic within range (raw subtraction of two timed
        // fractions with 10^9 denominators overflows i64 in num * den).
        self.get_limited_time_f64(bar_to) - self.get_limited_time_f64(bar_from)
    }

    fn get_limited_time_f64(&mut self, bar: Fraction) -> f64 {
        if let Some(time) = self.time_f64_cache.get(&bar) {
            return *time;
        }
        let time = self.get_time(bar).limit_denominator(1_000_000).to_f64();
        self.time_f64_cache.insert(bar, time);
        time
    }

    fn time_at(timed: &[(Fraction, Event)], bar: Fraction) -> Fraction {
        let idx = Self::timed_event_index(timed, bar);
        let (ref t, ref e) = timed[idx];
        Self::time_from_timed_event(*t, e, bar)
    }

    fn timed_event_index(timed: &[(Fraction, Event)], bar: Fraction) -> usize {
        let after = timed.partition_point(|probe| probe.1.bar <= bar);
        if after == 0 { 0 } else { after - 1 }
    }

    fn time_from_timed_event(t: Fraction, event: &Event, bar: Fraction) -> Fraction {
        let bpm = event.bpm.unwrap_or(Fraction::from_integer(120));
        let bar_length = event.bar_length.unwrap_or(Fraction::from_integer(4));
        let delta = bar - event.bar;
        (t + bar_length * Fraction::from_integer(60) / bpm * delta).limit_denominator(1_000_000_000)
    }

    /// Inverse: get bar position from elapsed time
    pub fn get_bar_by_time(&mut self, time: f64) -> Fraction {
        let mut t: f64 = 0.0;
        let mut event = Event::new(Fraction::zero());
        event.bpm = Some(Fraction::from_integer(120));
        event.bar_length = Some(Fraction::from_integer(4));
        event.sentence_length = Some(4);

        let events = self.events.clone();
        for i in 0..events.len() {
            event = event.merge(&events[i]);
            if i + 1 == events.len() {
                break;
            }

            let bpm = event.bpm.unwrap_or(Fraction::from_integer(120));
            let bar_length = event.bar_length.unwrap_or(Fraction::from_integer(4));
            let event_time = (bar_length * Fraction::from_integer(60) / bpm
                * (events[i + 1].bar - event.bar))
                .to_f64();

            if t + event_time > time {
                break;
            } else {
                t += event_time;
            }
        }

        let bpm = event.bpm.unwrap_or(Fraction::from_integer(120));
        let bar_length = event.bar_length.unwrap_or(Fraction::from_integer(4));
        let beats_per_second = bpm / (bar_length * Fraction::from_integer(60));
        let delta_time = time - t;
        let bar = event.bar + Fraction::from_f64(delta_time) * beats_per_second;

        bar.limit_denominator(1000000)
    }
}

impl std::str::FromStr for Score {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Score::parse(s))
    }
}

impl Default for Score {
    fn default() -> Self {
        Score::new()
    }
}

fn invalid_note_event(note: &NoteData) -> Event {
    let text = if note.lane() == 0 {
        "SKILL"
    } else if note.note_type() == 1 {
        "FEVER CHANCE!"
    } else {
        "SUPER FEVER!!"
    };
    Event::new(note.bar()).with_text(text.to_string())
}

fn notes_share_slot(left: &NoteData, right: &NoteData) -> bool {
    left.bar() == right.bar() && left.lane() == right.lane() && left.width() == right.width()
}

fn events_equal(a: &Event, b: &Event) -> bool {
    a.bar == b.bar
        && a.bpm == b.bpm
        && a.bar_length == b.bar_length
        && a.sentence_length == b.sentence_length
        && a.speed == b.speed
        && a.section == b.section
        && a.text == b.text
}

fn path_looks_json(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

fn content_looks_json(content: &str) -> bool {
    content
        .trim_start()
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '{' | '['))
}

fn invalid_json_data(error: ScoreJsonError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
