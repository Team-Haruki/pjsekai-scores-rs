use std::collections::HashMap;
use std::fs;

use crate::fraction::Fraction;
use crate::line::{
    BpmDefinition, BpmReference, Line, ParsedItem, SpeedControl, SpeedDefinition, TicksPerBeat,
};
use crate::meta::Meta;
use crate::notes::event::Event;
use crate::notes::slide::SlideType;
use crate::notes::{NO_NOTE, NoteData, NoteIdx};

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
}

impl Score {
    pub fn new() -> Self {
        Score {
            meta: Meta::new(),
            notes: Vec::new(),
            active_notes: Vec::new(),
            events: Vec::new(),
            timed_events_cache: None,
        }
    }

    /// Open and parse a .sus file
    pub fn open(path: &str) -> std::io::Result<Score> {
        let content = fs::read_to_string(path)?;
        let mut score = Score::new();
        let lines: Vec<Line> = content.lines().map(Line::new).collect();
        score.init_by_lines(&lines);
        Ok(score)
    }

    /// Parse from string content
    pub fn parse(content: &str) -> Score {
        let mut score = Score::new();
        let lines: Vec<Line> = content.lines().map(Line::new).collect();
        score.init_by_lines(&lines);
        score
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
        // Sort notes by bar position
        self.notes.sort_by(|a, b| {
            a.bar()
                .partial_cmp(&b.bar())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let n = self.notes.len();
        let mut note_deleted = vec![false; n];
        let mut note_indexes: HashMap<Fraction, Vec<usize>> = HashMap::new();

        // Pass 1: Filter invalid notes (outside lanes 2-13), create index
        for (i, is_deleted) in note_deleted.iter_mut().enumerate() {
            let lane = self.notes[i].lane();
            if !(0..12).contains(&(lane - 2)) {
                *is_deleted = true;
                let bar = self.notes[i].bar();
                let note_type = self.notes[i].note_type();
                let text = if lane == 0 {
                    "SKILL".to_string()
                } else if note_type == 1 {
                    "FEVER CHANCE!".to_string()
                } else {
                    "SUPER FEVER!!".to_string()
                };
                self.events.push(Event::new(bar).with_text(text));
                continue;
            }

            note_indexes.entry(self.notes[i].bar()).or_default().push(i);
        }

        // Pass 2: Associate Directional notes with Tap notes
        for i in 0..n {
            if note_deleted[i] || !self.notes[i].is_directional() {
                continue;
            }

            let dir_bar = self.notes[i].bar();
            let dir_lane = self.notes[i].lane();
            let dir_width = self.notes[i].width();

            if let Some(indexes) = note_indexes.get(&dir_bar) {
                for &j in indexes {
                    if note_deleted[j] || !self.notes[j].is_tap() {
                        continue;
                    }

                    let tap = &self.notes[j];
                    if tap.bar() == dir_bar && tap.lane() == dir_lane && tap.width() == dir_width {
                        note_deleted[j] = true;
                        if let NoteData::Directional(_, ref mut d) = self.notes[i] {
                            d.tap_idx = j;
                        }
                    }
                }
            }
        }

        // Pass 3: Associate Tap/Directional with Slide notes + chain slides
        for i in 0..n {
            if note_deleted[i] || !self.notes[i].is_slide() {
                continue;
            }

            // Set head to self if not set
            if let NoteData::Slide(_, ref s) = self.notes[i]
                && s.head_idx == NO_NOTE
                && let NoteData::Slide(_, ref mut s) = self.notes[i]
            {
                s.head_idx = i;
            }

            let slide_bar = self.notes[i].bar();
            let slide_lane = self.notes[i].lane();
            let slide_width = self.notes[i].width();

            // Find matching Tap
            if let Some(indexes) = note_indexes.get(&slide_bar).cloned() {
                for j in &indexes {
                    let j = *j;
                    if note_deleted[j] || !self.notes[j].is_tap() {
                        continue;
                    }
                    let tap = &self.notes[j];
                    if tap.bar() == slide_bar
                        && tap.lane() == slide_lane
                        && tap.width() == slide_width
                    {
                        note_deleted[j] = true;
                        if let NoteData::Slide(_, ref mut s) = self.notes[i] {
                            s.tap_idx = j;
                        }
                    }
                }

                // Find matching Directional
                for j in &indexes {
                    let j = *j;
                    if note_deleted[j] || !self.notes[j].is_directional() {
                        continue;
                    }
                    let dir = &self.notes[j];
                    if dir.bar() == slide_bar
                        && dir.lane() == slide_lane
                        && dir.width() == slide_width
                    {
                        note_deleted[j] = true;
                        let dir_tap_idx = self.notes[j]
                            .as_directional()
                            .map(|d| d.tap_idx)
                            .unwrap_or(NO_NOTE);
                        if let NoteData::Slide(_, ref mut s) = self.notes[i] {
                            s.directional_idx = j;
                            if dir_tap_idx != NO_NOTE {
                                s.tap_idx = dir_tap_idx;
                            }
                        }
                    }
                }
            }

            // Chain slides: find next slide with same channel and decoration
            let (slide_type, channel, decoration) = if let NoteData::Slide(_, ref s) = self.notes[i]
            {
                (self.notes[i].note_type(), s.channel, s.decoration)
            } else {
                continue;
            };

            if !matches!(SlideType::from_i32(slide_type), Some(SlideType::End)) {
                let head_idx = if let NoteData::Slide(_, ref s) = self.notes[i] {
                    s.head_idx
                } else {
                    NO_NOTE
                };

                for (j, &is_deleted_j) in note_deleted.iter().enumerate().skip(i + 1) {
                    if is_deleted_j || !self.notes[j].is_slide() {
                        continue;
                    }
                    if let NoteData::Slide(_, ref next_s) = self.notes[j]
                        && next_s.channel == channel
                        && next_s.decoration == decoration
                    {
                        // Set next pointer
                        if let NoteData::Slide(_, ref mut s) = self.notes[i] {
                            s.next_idx = j;
                        }
                        // Set head pointer on next
                        if let NoteData::Slide(_, ref mut next_s) = self.notes[j] {
                            next_s.head_idx = head_idx;
                        }
                        break;
                    }
                }
            }
        }

        // Build active notes list (non-deleted)
        self.active_notes = (0..n).filter(|&i| !note_deleted[i]).collect();
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
        let timed = self.timed_events().to_vec();
        let after = timed.partition_point(|probe| probe.1.bar <= bar);
        let idx = if after == 0 { 0 } else { after - 1 };

        let (ref t, ref e) = timed[idx];
        let bpm = e.bpm.unwrap_or(Fraction::from_integer(120));
        let bar_length = e.bar_length.unwrap_or(Fraction::from_integer(4));
        let delta = bar - e.bar;
        let time = (*t + bar_length * Fraction::from_integer(60) / bpm * delta)
            .limit_denominator(1_000_000_000);
        (time, e.clone())
    }

    pub fn get_time(&mut self, bar: Fraction) -> Fraction {
        self.get_timed_event(bar).0
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
        let t_to = self.get_time(bar_to).limit_denominator(1_000_000);
        let t_from = self.get_time(bar_from).limit_denominator(1_000_000);
        (t_to - t_from).to_f64()
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

fn events_equal(a: &Event, b: &Event) -> bool {
    a.bar == b.bar
        && a.bpm == b.bpm
        && a.bar_length == b.bar_length
        && a.sentence_length == b.sentence_length
        && a.speed == b.speed
        && a.section == b.section
        && a.text == b.text
}
