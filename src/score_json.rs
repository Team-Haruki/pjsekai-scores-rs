use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fmt;

use serde_json::{Map, Value};

use crate::fraction::Fraction;
use crate::meta::Meta;
use crate::notes::directional::{Directional, DirectionalType};
use crate::notes::event::Event;
use crate::notes::slide::{Slide, SlideType};
use crate::notes::tap::{Tap, TapType};
use crate::notes::{NO_NOTE, NoteBase, NoteData, NoteIdx};
use crate::score::Score;

const DEFAULT_TICKS_PER_BEAT: i64 = 480;
const DEFAULT_BEATS_PER_MEASURE: f64 = 4.0;

#[derive(Debug)]
pub enum ScoreJsonError {
    Json(serde_json::Error),
    ExpectedObject,
}

impl fmt::Display for ScoreJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScoreJsonError::Json(e) => write!(f, "{e}"),
            ScoreJsonError::ExpectedObject => write!(f, "expected a JSON object"),
        }
    }
}

impl Error for ScoreJsonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ScoreJsonError::Json(e) => Some(e),
            ScoreJsonError::ExpectedObject => None,
        }
    }
}

impl From<serde_json::Error> for ScoreJsonError {
    fn from(value: serde_json::Error) -> Self {
        ScoreJsonError::Json(value)
    }
}

#[derive(Clone, Copy)]
struct TickSegment {
    tick: i64,
    bar: Fraction,
    tick_length: i64,
}

struct TickConverter {
    segments: Vec<TickSegment>,
}

impl TickConverter {
    fn new(events: &[Value], ticks_per_beat: i64) -> Self {
        let default_tick_length = ticks_per_beat * DEFAULT_BEATS_PER_MEASURE as i64;
        let mut segments = vec![TickSegment {
            tick: 0,
            bar: Fraction::zero(),
            tick_length: default_tick_length,
        }];

        let mut current_tick = 0;
        let mut current_bar = Fraction::zero();
        let mut current_tick_length = default_tick_length;

        let mut signature_events: Vec<&Value> = events
            .iter()
            .filter(|event| int_field(event, "eventType", -1) == 3)
            .collect();
        signature_events
            .sort_by_key(|event| (int_field(event, "ticks", 0), int_field(event, "id", 0)));

        for event in signature_events {
            let tick = int_field(event, "ticks", 0);
            if tick < current_tick {
                continue;
            }

            current_bar =
                current_bar + Fraction::new(tick - current_tick, current_tick_length.max(1));
            current_tick = tick;
            current_tick_length =
                time_signature_to_tick_length(event.get("changeValue"), ticks_per_beat);

            if let Some(last) = segments.last_mut()
                && last.tick == tick
            {
                last.bar = current_bar;
                last.tick_length = current_tick_length;
                continue;
            }

            segments.push(TickSegment {
                tick,
                bar: current_bar,
                tick_length: current_tick_length,
            });
        }

        Self { segments }
    }

    fn to_bar(&self, ticks: i64) -> Fraction {
        let index = self
            .segments
            .partition_point(|segment| segment.tick <= ticks)
            .saturating_sub(1);
        let segment = self.segments[index];
        segment.bar + Fraction::new(ticks - segment.tick, segment.tick_length.max(1))
    }
}

#[derive(Debug, Clone)]
struct RawNote {
    id: i64,
    ticks: i64,
    lane_start: i32,
    lane_end: i32,
    category: i32,
    note_base_type: i32,
    previous_connection_id: i64,
    next_connection_id: i64,
    direction: i32,
    note_line_type: i32,
    speed_ratio: f64,
    critical: bool,
    is_skip_false: bool,
}

type NoteSlotKey = (i64, i32, i32);
type TapSlotKey = (i64, i32);

struct JsonNoteBuilder<'a> {
    tick_converter: &'a TickConverter,
    notes: Vec<NoteData>,
    active_notes: Vec<NoteIdx>,
}

impl<'a> JsonNoteBuilder<'a> {
    fn new(tick_converter: &'a TickConverter) -> Self {
        Self {
            tick_converter,
            notes: Vec::new(),
            active_notes: Vec::new(),
        }
    }

    fn finish(mut self) -> (Vec<NoteData>, Vec<NoteIdx>) {
        self.active_notes.sort_by(|&left, &right| {
            let left_note = &self.notes[left];
            let right_note = &self.notes[right];
            left_note
                .bar()
                .cmp(&right_note.bar())
                .then(left_note.lane().cmp(&right_note.lane()))
        });
        (self.notes, self.active_notes)
    }

    fn push_active(&mut self, note: NoteData) -> NoteIdx {
        let idx = self.notes.len();
        self.notes.push(note);
        self.active_notes.push(idx);
        idx
    }

    fn push_attached(&mut self, note: NoteData) -> NoteIdx {
        let idx = self.notes.len();
        self.notes.push(note);
        idx
    }

    fn make_base(&self, note: &RawNote, lane_note: &RawNote, note_type: i32) -> NoteBase {
        let mut base = NoteBase::new(
            self.tick_converter.to_bar(note.ticks),
            note_lane(lane_note),
            note_width(lane_note),
            note_type,
        );
        base.speed = note_speed_ratio(note);
        base
    }

    fn make_tap(&self, note: &RawNote, lane_note: &RawNote, tap_type: i32) -> NoteData {
        NoteData::Tap(self.make_base(note, lane_note, tap_type), Tap)
    }

    fn make_directional(
        &self,
        note: &RawNote,
        lane_note: &RawNote,
        directional_type: i32,
        tap_idx: NoteIdx,
    ) -> NoteData {
        NoteData::Directional(
            self.make_base(note, lane_note, directional_type),
            Directional { tap_idx },
        )
    }

    fn make_slide(
        &self,
        note: &RawNote,
        lane_note: &RawNote,
        slide_type: i32,
        channel: i32,
        decoration: bool,
    ) -> NoteData {
        NoteData::Slide(
            self.make_base(note, lane_note, slide_type),
            Slide::new(channel, decoration),
        )
    }

    fn push_attached_tap(&mut self, note: &RawNote, lane_note: &RawNote, tap_type: i32) -> NoteIdx {
        let tap = self.make_tap(note, lane_note, tap_type);
        self.push_attached(tap)
    }

    fn push_attached_directional(
        &mut self,
        note: &RawNote,
        lane_note: &RawNote,
        directional_type: i32,
        tap_idx: NoteIdx,
    ) -> NoteIdx {
        let directional = self.make_directional(note, lane_note, directional_type, tap_idx);
        self.push_attached(directional)
    }
}

pub fn parse_score_json(content: &str) -> Result<Score, ScoreJsonError> {
    let value: Value = serde_json::from_str(content)?;
    score_from_value(&value)
}

pub fn score_from_value(data: &Value) -> Result<Score, ScoreJsonError> {
    let data_object = data.as_object().ok_or(ScoreJsonError::ExpectedObject)?;
    let chart = data
        .get("chart")
        .and_then(Value::as_object)
        .unwrap_or(data_object);

    let event_data = array_field(chart, "MusicScoreEventDataList");
    let note_data = array_field(chart, "NoteList");
    let ticks_per_beat = int_field(data, "ticksPerBeat", DEFAULT_TICKS_PER_BEAT).max(1);
    let tick_converter = TickConverter::new(event_data, ticks_per_beat);

    let mut score = Score::new();
    score.meta = parse_meta(data, chart);
    init_events_by_data(&mut score, event_data, &tick_converter);
    merge_json_events_by_bar(&mut score);

    let (notes, active_notes) = parse_notes(note_data, &tick_converter);
    score.notes = notes;
    score.active_notes = active_notes;
    score.init_events();

    Ok(score)
}

fn init_events_by_data(score: &mut Score, events: &[Value], tick_converter: &TickConverter) {
    let mut sorted: Vec<&Value> = events.iter().collect();
    sorted.sort_by_key(|event| {
        (
            int_field(event, "ticks", 0),
            int_field(event, "eventType", -1),
        )
    });

    for event in sorted {
        let bar = tick_converter.to_bar(int_field(event, "ticks", 0));
        match int_field(event, "eventType", -1) {
            0 => score
                .events
                .push(Event::new(bar).with_bpm(Fraction::from_f64(float_field(
                    event,
                    "changeValue",
                    120.0,
                )))),
            1 => score.events.push(Event::new(bar).with_speed(float_field(
                event,
                "changeValue",
                1.0,
            ))),
            2 => {}
            3 => score.events.push(
                Event::new(bar)
                    .with_bar_length(time_signature_to_bar_length(event.get("changeValue"))),
            ),
            _ => {}
        }
    }
}

fn parse_notes(
    note_data: &[Value],
    tick_converter: &TickConverter,
) -> (Vec<NoteData>, Vec<NoteIdx>) {
    let mut raw_notes: Vec<RawNote> = note_data.iter().map(read_note).collect();
    raw_notes.sort_by(|left, right| {
        left.ticks
            .cmp(&right.ticks)
            .then(left.lane_start.cmp(&right.lane_start))
            .then(left.id.cmp(&right.id))
    });

    let (chains, connected_ids) = build_chains(&raw_notes);
    let mut connected_slide_slots = HashSet::new();
    let mut critical_slide_slots = HashSet::new();
    let mut hidden_head_slide_slots = HashSet::new();
    let mut standalone_tap_slots = HashSet::new();
    let mut occupied_tap_slots = HashSet::new();

    for chain in &chains {
        for &note_index in chain {
            connected_slide_slots.insert(note_slot_key(&raw_notes[note_index]));
        }
    }

    for raw_chain in &chains {
        let chain = remove_adjacent_visible_relay_duplicates(&raw_notes, raw_chain);
        let visible_relay_as_attachment = chain_has_curve_line(&raw_notes, &chain);
        for (index, &note_index) in chain.iter().enumerate() {
            let note = &raw_notes[note_index];
            let slide_type = get_slide_type(note, index + 1 == chain.len());
            if !is_visible_relay_attachment(note)
                && connected_note_adds_tap(note, slide_type, visible_relay_as_attachment)
            {
                reserve_tap_slot(&mut occupied_tap_slots, note);
            }
        }
    }

    for note in &raw_notes {
        if !connected_ids.contains(&note.id) {
            reserve_tap_slot(&mut occupied_tap_slots, note);
            standalone_tap_slots.insert(note_slot_key(note));
        }
    }

    for chain in &chains {
        if is_hidden_head_slide_chain(&raw_notes, chain, &standalone_tap_slots) {
            hidden_head_slide_slots.insert(note_slot_key(&raw_notes[chain[0]]));
        }
    }

    let mut builder = JsonNoteBuilder::new(tick_converter);
    let mut channel_available_at = [f64::NEG_INFINITY; 36];

    for raw_chain in &chains {
        let chain = remove_adjacent_visible_relay_duplicates(&raw_notes, raw_chain);
        if chain.is_empty() {
            continue;
        }

        let start_tick = raw_notes[chain[0]].ticks as f64;
        let end_tick = raw_notes[*chain.last().unwrap()].ticks as f64;
        let channel = match channel_available_at
            .iter()
            .position(|&available_at| available_at < start_tick)
        {
            Some(index) => index,
            None => channel_available_at
                .iter()
                .enumerate()
                .min_by(|(_, left), (_, right)| left.total_cmp(right))
                .map(|(index, _)| index)
                .unwrap_or(0),
        };
        channel_available_at[channel] = channel_available_at[channel].max(end_tick);

        let chain_decoration = is_decoration_slide_chain(&raw_notes, &chain);
        let visible_relay_as_attachment = chain_has_curve_line(&raw_notes, &chain);
        let is_hidden_head = is_hidden_head_slide_chain(&raw_notes, &chain, &standalone_tap_slots);
        let mut previous_slide = NO_NOTE;
        let mut head_slide = NO_NOTE;
        let mut deferred_critical_taps = Vec::new();

        for (index, &note_index) in chain.iter().enumerate() {
            let note = &raw_notes[note_index];
            let base = note.note_base_type;
            let decoration = chain_decoration || is_decoration_slide_note(note);
            let slide_type = get_slide_type(note, index + 1 == chain.len());
            let output_note =
                if slide_type == SlideType::Relay as i32 && is_visible_relay_attachment(note) {
                    with_visible_relay_attachment_slot(note, &mut occupied_tap_slots)
                } else {
                    note.clone()
                };

            let slide =
                builder.make_slide(note, &output_note, slide_type, channel as i32, decoration);
            let slide_idx = builder.push_active(slide);

            if head_slide == NO_NOTE {
                head_slide = slide_idx;
            }
            if let Some(slide) = builder.notes[slide_idx].as_slide_mut() {
                slide.head_idx = head_slide;
            }
            if previous_slide != NO_NOTE {
                if let Some(previous) = builder.notes[previous_slide].as_slide_mut() {
                    previous.next_idx = slide_idx;
                }
            }

            attach_connected_note(
                &mut builder,
                slide_idx,
                note,
                &output_note,
                is_hidden_head && index == 0 && (base == 9 || base == 12),
                &mut deferred_critical_taps,
                &mut critical_slide_slots,
            );

            previous_slide = slide_idx;
        }

        for note in deferred_critical_taps {
            if !standalone_tap_slots.contains(&note_slot_key(&note)) {
                let tap = builder.make_tap(&note, &note, TapType::CriticalCancel as i32);
                builder.push_active(tap);
            }
        }

        let _ = visible_relay_as_attachment;
    }

    for note in &raw_notes {
        if !connected_ids.contains(&note.id) {
            add_standalone_note(
                &mut builder,
                note,
                &connected_slide_slots,
                &critical_slide_slots,
                &hidden_head_slide_slots,
            );
        }
    }

    builder.finish()
}

fn read_note(js: &Value) -> RawNote {
    RawNote {
        id: int_field(js, "id", 0),
        ticks: int_field(js, "ticks", 0),
        lane_start: int_field(js, "laneStart", 0) as i32,
        lane_end: int_field(js, "laneEnd", 0) as i32,
        category: int_field(js, "category", 0) as i32,
        note_base_type: int_field(js, "noteBaseType", 0) as i32,
        previous_connection_id: int_field(js, "previousConnectionId", -1),
        next_connection_id: int_field(js, "nextConnectionId", -1),
        direction: int_field(js, "direction", 0) as i32,
        note_line_type: int_field(js, "noteLineType", 0) as i32,
        speed_ratio: sanitize_speed_ratio(float_field(js, "speedRatio", 1.0)),
        critical: truthy_field(js, "type"),
        is_skip_false: matches!(js.get("isSkip"), Some(Value::Bool(false))),
    }
}

fn build_chains(notes: &[RawNote]) -> (Vec<Vec<usize>>, HashSet<i64>) {
    let by_id: HashMap<i64, usize> = notes
        .iter()
        .enumerate()
        .map(|(index, note)| (note.id, index))
        .collect();
    let mut visited = HashSet::new();
    let mut chains = Vec::new();

    for (index, note) in notes.iter().enumerate() {
        if visited.contains(&note.id) {
            continue;
        }
        if note.next_connection_id == -1 && note.previous_connection_id == -1 {
            continue;
        }
        if note.previous_connection_id != -1 {
            continue;
        }

        let mut chain = Vec::new();
        let mut current = Some(index);
        while let Some(current_index) = current {
            let current_note = &notes[current_index];
            if visited.contains(&current_note.id) {
                break;
            }

            chain.push(current_index);
            visited.insert(current_note.id);
            current = if current_note.next_connection_id == -1 {
                None
            } else {
                by_id.get(&current_note.next_connection_id).copied()
            };
        }
        if !chain.is_empty() {
            chains.push(chain);
        }
    }

    for (index, note) in notes.iter().enumerate() {
        if !visited.contains(&note.id)
            && (note.next_connection_id != -1 || note.previous_connection_id != -1)
        {
            chains.push(vec![index]);
            visited.insert(note.id);
        }
    }

    (chains, visited)
}

fn add_standalone_note(
    builder: &mut JsonNoteBuilder<'_>,
    note: &RawNote,
    connected_slide_slots: &HashSet<NoteSlotKey>,
    critical_slide_slots: &HashSet<NoteSlotKey>,
    hidden_head_slide_slots: &HashSet<NoteSlotKey>,
) {
    if note.note_base_type == 3 || note.category == 3 {
        let tap_type = standalone_tap_type(
            note,
            connected_slide_slots,
            critical_slide_slots,
            hidden_head_slide_slots,
        );
        let tap_idx = builder.push_attached_tap(note, note, tap_type);
        let directional = builder.make_directional(
            note,
            note,
            direction_to_directional(note.direction),
            tap_idx,
        );
        builder.push_active(directional);
        return;
    }

    if note.note_base_type == 4 || note.category == 8 {
        let tap_type = tap_type_from_json(note, TapType::Tap as i32);
        if matches!(note.direction, 1 | 2)
            || tap_type == TapType::Trend as i32
            || tap_type == TapType::CriticalTrend as i32
        {
            let tap_idx = builder.push_attached_tap(note, note, tap_type);
            let directional = builder.make_directional(
                note,
                note,
                direction_to_directional(note.direction),
                tap_idx,
            );
            builder.push_active(directional);
        } else {
            let tap = builder.make_tap(note, note, tap_type);
            builder.push_active(tap);
        }
        return;
    }

    let tap_type = standalone_tap_type(
        note,
        connected_slide_slots,
        critical_slide_slots,
        hidden_head_slide_slots,
    );
    let tap = builder.make_tap(note, note, tap_type);
    builder.push_active(tap);
}

fn attach_connected_note(
    builder: &mut JsonNoteBuilder<'_>,
    slide_idx: NoteIdx,
    note: &RawNote,
    output_note: &RawNote,
    is_hidden_head_start: bool,
    deferred_critical_taps: &mut Vec<RawNote>,
    critical_slide_slots: &mut HashSet<NoteSlotKey>,
) {
    let base = note.note_base_type;
    let (slide_type, decoration) = match &builder.notes[slide_idx] {
        NoteData::Slide(base, slide) => (base.note_type, slide.decoration),
        _ => return,
    };

    let mut tap_idx = NO_NOTE;
    let mut directional_idx = NO_NOTE;

    if decoration && note.critical && (base == 10 || base == 13) {
        tap_idx = builder.push_attached_tap(note, output_note, TapType::CriticalCancel as i32);
        deferred_critical_taps.push(output_note.clone());
        critical_slide_slots.insert(note_slot_key(output_note));
    } else if is_hidden_head_start && (base == 9 || base == 12) {
        tap_idx = builder.push_attached_tap(
            note,
            output_note,
            if note.critical {
                TapType::CriticalCancel as i32
            } else {
                TapType::Cancel as i32
            },
        );
    } else if slide_type == SlideType::Relay as i32 && is_visible_relay_attachment(note) {
        tap_idx = builder.push_attached_tap(note, output_note, TapType::Flick as i32);
    } else if base == 3 || note.category == 3 {
        if note.critical {
            tap_idx = builder.push_attached_tap(note, output_note, TapType::Critical as i32);
        }
        directional_idx = builder.push_attached_directional(
            note,
            output_note,
            direction_to_directional(note.direction),
            tap_idx,
        );
    } else if matches!(base, 8 | 11 | 9 | 12) {
        tap_idx = builder.push_attached_tap(
            note,
            output_note,
            tap_type_from_json(note, TapType::Tap as i32),
        );
    } else if note.critical && (base == 1 || base == 2) {
        tap_idx = builder.push_attached_tap(note, output_note, TapType::Critical as i32);
    }

    let line_directional_type = line_type_to_directional(note.note_line_type);
    if line_directional_type != 0 {
        directional_idx =
            builder.push_attached_directional(note, output_note, line_directional_type, tap_idx);
    }

    if let Some(slide) = builder.notes[slide_idx].as_slide_mut() {
        if tap_idx != NO_NOTE {
            slide.tap_idx = tap_idx;
        }
        if directional_idx != NO_NOTE {
            slide.directional_idx = directional_idx;
        }
    }
}

fn remove_adjacent_visible_relay_duplicates(notes: &[RawNote], chain: &[usize]) -> Vec<usize> {
    let mut filtered = Vec::with_capacity(chain.len());
    for (index, &note_index) in chain.iter().enumerate() {
        let note = &notes[note_index];
        let next = chain.get(index + 1).map(|&next_index| &notes[next_index]);
        if let Some(next) = next
            && is_visible_relay_slide_note(note)
            && is_visible_relay_slide_note(next)
            && (next.ticks - note.ticks).abs() <= 1
        {
            continue;
        }
        filtered.push(note_index);
    }
    filtered
}

fn with_visible_relay_attachment_slot(
    note: &RawNote,
    occupied_tap_slots: &mut HashSet<TapSlotKey>,
) -> RawNote {
    if !occupied_tap_slots.contains(&tap_slot_key(note)) {
        occupied_tap_slots.insert(tap_slot_key(note));
        return note.clone();
    }

    let width = note_width(note);
    let max_lane_start = (12 - width).max(0);
    let mut candidates: Vec<i32> = (0..=max_lane_start).collect();
    candidates.sort_by_key(|candidate| (*candidate - note.lane_start).abs());

    let lane_start = candidates
        .into_iter()
        .find(|candidate| !occupied_tap_slots.contains(&(note.ticks, candidate + 2)));
    let Some(lane_start) = lane_start else {
        return note.clone();
    };

    let mut placed = note.clone();
    placed.lane_start = lane_start;
    placed.lane_end = lane_start + width - 1;
    occupied_tap_slots.insert(tap_slot_key(&placed));
    placed
}

fn reserve_tap_slot(occupied_tap_slots: &mut HashSet<TapSlotKey>, note: &RawNote) {
    occupied_tap_slots.insert(tap_slot_key(note));
}

fn get_slide_type(note: &RawNote, is_last: bool) -> i32 {
    let base = note.note_base_type;
    if is_last {
        return SlideType::End as i32;
    }
    if matches!(base, 2 | 8 | 9 | 10) {
        return SlideType::Start as i32;
    }
    if matches!(base, 1 | 3 | 11 | 12 | 13) {
        return SlideType::End as i32;
    }
    if base == 6 || base == 14 || note.category == 11 {
        return SlideType::Invisible as i32;
    }
    SlideType::Relay as i32
}

fn connected_note_adds_tap(
    note: &RawNote,
    slide_type: i32,
    visible_relay_as_attachment: bool,
) -> bool {
    let base = note.note_base_type;
    (slide_type == SlideType::Relay as i32
        && visible_relay_as_attachment
        && is_visible_relay_slide_note(note))
        || base == 3
        || note.category == 3
        || base == 8
        || base == 11
        || base == 9
        || base == 12
        || (note.critical && (base == 1 || base == 2))
}

fn standalone_tap_type(
    note: &RawNote,
    connected_slide_slots: &HashSet<NoteSlotKey>,
    _critical_slide_slots: &HashSet<NoteSlotKey>,
    hidden_head_slide_slots: &HashSet<NoteSlotKey>,
) -> i32 {
    let tap_type = tap_type_from_json(note, TapType::Tap as i32);
    if tap_type == TapType::Trend as i32 || tap_type == TapType::CriticalTrend as i32 {
        return tap_type;
    }
    if hidden_head_slide_slots.contains(&note_slot_key(note)) {
        return tap_type;
    }
    if connected_slide_slots.contains(&note_slot_key(note)) {
        return downgrade_critical_tap_type(tap_type);
    }
    tap_type
}

fn tap_type_from_json(note: &RawNote, fallback: i32) -> i32 {
    let base = note.note_base_type;
    let category = note.category;
    if category == 6 {
        return critical_pair(note.critical, TapType::CriticalTrend, TapType::Trend);
    }
    if base == 11 {
        return critical_pair(note.critical, TapType::CriticalTrend, TapType::Trend);
    }
    if base == 8 {
        return critical_pair(note.critical, TapType::Critical, TapType::Tap);
    }
    if base == 9 || base == 12 {
        return critical_pair(note.critical, TapType::CriticalCancel, TapType::Cancel);
    }
    if base == 4 {
        return critical_pair(note.critical, TapType::CriticalTrend, TapType::Trend);
    }
    if base == 3 {
        return critical_pair(note.critical, TapType::Critical, TapType::Tap);
    }
    if base == 1 || base == 2 {
        return critical_pair(note.critical, TapType::Critical, TapType::Tap);
    }
    fallback
}

fn critical_pair(critical: bool, critical_type: TapType, normal_type: TapType) -> i32 {
    if critical {
        critical_type as i32
    } else {
        normal_type as i32
    }
}

fn downgrade_critical_tap_type(tap_type: i32) -> i32 {
    if tap_type == TapType::Critical as i32 {
        TapType::Tap as i32
    } else if tap_type == TapType::CriticalTrend as i32 {
        TapType::Trend as i32
    } else if tap_type == TapType::CriticalCancel as i32 {
        TapType::Cancel as i32
    } else {
        tap_type
    }
}

fn direction_to_directional(direction: i32) -> i32 {
    match direction {
        1 => DirectionalType::UpperLeft as i32,
        2 => DirectionalType::UpperRight as i32,
        _ => DirectionalType::Up as i32,
    }
}

fn line_type_to_directional(line_type: i32) -> i32 {
    match line_type {
        1 => DirectionalType::LowerLeft as i32,
        2 => DirectionalType::Down as i32,
        _ => 0,
    }
}

fn is_hidden_head_slide_chain(
    notes: &[RawNote],
    chain: &[usize],
    standalone_tap_slots: &HashSet<NoteSlotKey>,
) -> bool {
    let Some(&first_index) = chain.first() else {
        return false;
    };
    let Some(&last_index) = chain.last() else {
        return false;
    };
    let first = &notes[first_index];
    let last = &notes[last_index];
    first.note_base_type == 9
        && last.note_base_type == 12
        && last.category == 5
        && standalone_tap_slots.contains(&note_slot_key(first))
}

fn is_decoration_slide_chain(notes: &[RawNote], chain: &[usize]) -> bool {
    chain
        .iter()
        .any(|&note_index| is_decoration_slide_note(&notes[note_index]))
}

fn chain_has_curve_line(notes: &[RawNote], chain: &[usize]) -> bool {
    chain
        .iter()
        .any(|&note_index| notes[note_index].note_line_type != 0)
}

fn is_visible_relay_slide_note(note: &RawNote) -> bool {
    note.note_base_type == 5 || note.category == 2
}

fn should_visible_relay_affect_path(note: &RawNote) -> bool {
    is_visible_relay_slide_note(note) && note.is_skip_false
}

fn is_visible_relay_attachment(note: &RawNote) -> bool {
    is_visible_relay_slide_note(note) && !should_visible_relay_affect_path(note)
}

fn is_decoration_slide_note(note: &RawNote) -> bool {
    note.category == 9 || note.note_base_type == 10 || note.note_base_type == 13
}

fn note_slot_key(note: &RawNote) -> NoteSlotKey {
    (note.ticks, note_lane(note), note_width(note))
}

fn tap_slot_key(note: &RawNote) -> TapSlotKey {
    (note.ticks, note_lane(note))
}

fn note_lane(note: &RawNote) -> i32 {
    note.lane_start + 2
}

fn note_width(note: &RawNote) -> i32 {
    (note.lane_end - note.lane_start + 1).max(1)
}

fn note_speed_ratio(note: &RawNote) -> Option<f64> {
    let speed_ratio = sanitize_speed_ratio(note.speed_ratio);
    if (speed_ratio - 1.0).abs() > 0.0001 {
        Some(speed_ratio)
    } else {
        None
    }
}

fn sanitize_speed_ratio(speed_ratio: f64) -> f64 {
    if !speed_ratio.is_finite() || speed_ratio <= 0.0 {
        1.0
    } else {
        speed_ratio
    }
}

fn parse_meta(data: &Value, chart: &Map<String, Value>) -> Meta {
    let mut meta = Meta::new();
    meta.title = custom_info_string(data, "title")
        .or_else(|| string_field(data, "title"))
        .or_else(|| string_field_from_map(chart, "title"));
    meta.artist = custom_info_string(data, "artist").or_else(|| string_field(data, "artist"));
    meta.designer = custom_info_string(data, "author")
        .or_else(|| string_field(data, "author"))
        .or_else(|| string_field(data, "designer"));
    meta.difficulty = string_field(data, "musicDifficultyType")
        .or_else(|| string_field_from_map(chart, "musicDifficultyType"));
    meta.playlevel =
        string_field(data, "playLevel").or_else(|| string_field_from_map(chart, "playLevel"));
    meta.songid = string_field(data, "songId")
        .or_else(|| string_field(data, "musicId"))
        .or_else(|| string_field(data, "MusicId"))
        .or_else(|| string_field_from_map(chart, "songId"))
        .or_else(|| string_field_from_map(chart, "musicId"))
        .or_else(|| string_field_from_map(chart, "MusicId"));
    meta
}

fn merge_json_events_by_bar(score: &mut Score) {
    let mut merged: BTreeMap<Fraction, Event> = BTreeMap::new();
    for event in std::mem::take(&mut score.events) {
        merged
            .entry(event.bar)
            .and_modify(|existing| {
                existing.bpm = event.bpm.or(existing.bpm);
                existing.bar_length = event.bar_length.or(existing.bar_length);
                existing.sentence_length = event.sentence_length.or(existing.sentence_length);
                existing.speed = event.speed.or(existing.speed);
                existing.section = event.section.clone().or_else(|| existing.section.clone());
                existing.text = event.text.clone().or_else(|| existing.text.clone());
            })
            .or_insert(event);
    }
    score.events = merged.into_values().collect();
}

fn time_signature_to_bar_length(value: Option<&Value>) -> Fraction {
    if let Some(Value::String(raw)) = value {
        let trimmed = raw.trim();
        if let Some((numerator, denominator)) = trimmed.split_once('/')
            && let (Ok(numerator), Ok(denominator)) = (
                numerator.trim().parse::<f64>(),
                denominator.trim().parse::<f64>(),
            )
            && denominator > 0.0
        {
            return Fraction::from_f64(numerator * 4.0 / denominator);
        }
    }
    Fraction::from_f64(
        value
            .and_then(value_to_f64)
            .unwrap_or(DEFAULT_BEATS_PER_MEASURE),
    )
}

fn time_signature_to_tick_length(value: Option<&Value>, ticks_per_beat: i64) -> i64 {
    let beat_length = time_signature_to_bar_length(value).to_f64();
    ((beat_length * ticks_per_beat as f64).round() as i64).max(1)
}

fn array_field<'a>(object: &'a Map<String, Value>, key: &str) -> &'a [Value] {
    object
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn int_field(value: &Value, key: &str, fallback: i64) -> i64 {
    value.get(key).and_then(value_to_i64).unwrap_or(fallback)
}

fn value_to_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|value| value.round() as i64)),
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| value.round() as i64),
        _ => None,
    }
}

fn float_field(value: &Value, key: &str, fallback: f64) -> f64 {
    value.get(key).and_then(value_to_f64).unwrap_or(fallback)
}

fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn truthy_field(value: &Value, key: &str) -> bool {
    match value.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::String(s)) => !s.is_empty() && s != "0" && s != "false" && s != "False",
        _ => false,
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(value_to_string)
}

fn string_field_from_map(value: &Map<String, Value>, key: &str) -> Option<String> {
    value.get(key).and_then(value_to_string)
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    }
}

fn custom_info_string(data: &Value, key: &str) -> Option<String> {
    match data.get("userCustomMusicScoreInfoJson")? {
        Value::Object(info) => info.get(key).and_then(value_to_string),
        Value::String(raw) => serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|info| info.get(key).and_then(value_to_string)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_custom_chart_json() {
        let json = r#"
        {
          "title": "Example",
          "musicId": 42,
          "playLevel": 31,
          "musicDifficultyType": "master",
          "chart": {
            "MusicScoreEventDataList": [
              {"id": 1, "ticks": 0, "eventType": 0, "changeValue": 120},
              {"id": 2, "ticks": 1920, "eventType": 3, "changeValue": "3/4"},
              {"id": 3, "ticks": 3360, "eventType": 0, "changeValue": 180}
            ],
            "NoteList": [
              {"id": 1, "ticks": 0, "laneStart": 0, "laneEnd": 1, "category": 0, "type": 0, "noteBaseType": 1},
              {"id": 2, "ticks": 480, "laneStart": 2, "laneEnd": 3, "category": 3, "type": 1, "direction": 2, "noteBaseType": 3}
            ]
          }
        }
        "#;

        let mut score = parse_score_json(json).unwrap();
        assert_eq!(score.meta.title.as_deref(), Some("Example"));
        assert_eq!(score.meta.songid.as_deref(), Some("42"));
        assert_eq!(score.meta.playlevel.as_deref(), Some("31"));
        assert_eq!(score.meta.difficulty.as_deref(), Some("master"));
        assert_eq!(score.active_notes.len(), 2);
        assert_eq!(
            score.get_time(Fraction::from_integer(1)),
            Fraction::from_integer(2)
        );
        assert_eq!(score.get_time(Fraction::new(9, 4)), Fraction::new(15, 4));
    }

    #[test]
    fn connected_chain_uses_ts_slide_types() {
        let json = r#"
        {
          "MusicScoreEventDataList": [
            {"id": 1, "ticks": 0, "eventType": 0, "changeValue": 120}
          ],
          "NoteList": [
            {"id": 10, "ticks": 0, "laneStart": 0, "laneEnd": 1, "category": 1, "type": 0, "previousConnectionId": -1, "nextConnectionId": 11, "noteBaseType": 2},
            {"id": 11, "ticks": 480, "laneStart": 1, "laneEnd": 2, "category": 1, "type": 0, "previousConnectionId": 10, "nextConnectionId": 12, "noteBaseType": 2},
            {"id": 12, "ticks": 960, "laneStart": 2, "laneEnd": 3, "category": 1, "type": 0, "previousConnectionId": 11, "nextConnectionId": -1, "noteBaseType": 1}
          ]
        }
        "#;

        let score = parse_score_json(json).unwrap();
        let slide_types: Vec<i32> = score
            .active_notes
            .iter()
            .filter_map(|&idx| {
                score.notes[idx]
                    .is_slide()
                    .then(|| score.notes[idx].note_type())
            })
            .collect();
        assert_eq!(
            slide_types,
            vec![
                SlideType::Start as i32,
                SlideType::Start as i32,
                SlideType::End as i32
            ]
        );
    }

    #[test]
    fn standalone_cancel_notes_match_ts_output() {
        let json = r#"
        {
          "MusicScoreEventDataList": [
            {"id": 1, "ticks": 0, "eventType": 0, "changeValue": 120}
          ],
          "NoteList": [
            {"id": 1, "ticks": 0, "laneStart": 0, "laneEnd": 1, "category": 7, "type": 1, "noteBaseType": 9}
          ]
        }
        "#;

        let score = parse_score_json(json).unwrap();
        assert_eq!(score.active_notes.len(), 1);
        let note = &score.notes[score.active_notes[0]];
        assert!(note.is_none(&score.notes));
    }

    #[test]
    fn parses_trace_flick_notes() {
        let json = r#"
        {
          "MusicScoreEventDataList": [
            {"id": 1, "ticks": 0, "eventType": 0, "changeValue": 120}
          ],
          "NoteList": [
            {"id": 1, "ticks": 0, "laneStart": 0, "laneEnd": 1, "category": 8, "type": 1, "direction": 2, "noteBaseType": 4}
          ]
        }
        "#;

        let score = parse_score_json(json).unwrap();
        assert_eq!(score.active_notes.len(), 1);
        let note = &score.notes[score.active_notes[0]];
        assert!(note.is_directional());
        assert!(note.is_trend(&score.notes));
        assert!(note.is_critical(&score.notes));
    }

    #[test]
    fn relay_skip_notes_attach_tap_and_do_not_affect_path() {
        let json = r#"
        {
          "MusicScoreEventDataList": [
            {"id": 1, "ticks": 0, "eventType": 0, "changeValue": 120}
          ],
          "NoteList": [
            {"id": 1, "ticks": 0, "laneStart": 0, "laneEnd": 1, "category": 1, "type": 0, "previousConnectionId": -1, "nextConnectionId": 2, "noteBaseType": 2},
            {"id": 2, "ticks": 480, "laneStart": 1, "laneEnd": 2, "category": 2, "type": 0, "previousConnectionId": 1, "nextConnectionId": 3, "noteBaseType": 5},
            {"id": 3, "ticks": 481, "laneStart": 2, "laneEnd": 3, "category": 2, "type": 0, "previousConnectionId": 2, "nextConnectionId": 4, "noteBaseType": 5},
            {"id": 4, "ticks": 960, "laneStart": 3, "laneEnd": 4, "category": 2, "type": 0, "previousConnectionId": 3, "nextConnectionId": 5, "noteBaseType": 5, "isSkip": true},
            {"id": 5, "ticks": 1440, "laneStart": 4, "laneEnd": 5, "category": 1, "type": 0, "previousConnectionId": 4, "nextConnectionId": -1, "noteBaseType": 1}
          ]
        }
        "#;

        let score = parse_score_json(json).unwrap();
        let slides: Vec<_> = score
            .active_notes
            .iter()
            .copied()
            .filter(|&idx| score.notes[idx].is_slide())
            .collect();
        assert_eq!(slides.len(), 4);

        let skip_slide = slides
            .iter()
            .copied()
            .find(|&idx| score.notes[idx].bar() == Fraction::new(1, 2))
            .expect("skip relay slide");
        let slide = score.notes[skip_slide].as_slide().unwrap();
        assert_ne!(slide.tap_idx, NO_NOTE);
        assert!(!slide.is_path(score.notes[skip_slide].note_type()));
    }

    #[test]
    fn parses_guide_chains_as_decoration() {
        let json = r#"
        {
          "MusicScoreEventDataList": [
            {"id": 1, "ticks": 0, "eventType": 0, "changeValue": 120}
          ],
          "NoteList": [
            {"id": 1, "ticks": 0, "laneStart": 0, "laneEnd": 1, "category": 9, "type": 0, "previousConnectionId": -1, "nextConnectionId": 2, "noteBaseType": 10},
            {"id": 2, "ticks": 480, "laneStart": 4, "laneEnd": 5, "category": 9, "type": 0, "previousConnectionId": 1, "nextConnectionId": -1, "noteBaseType": 13}
          ]
        }
        "#;

        let score = parse_score_json(json).unwrap();
        let slides: Vec<_> = score
            .active_notes
            .iter()
            .filter_map(|&idx| score.notes[idx].as_slide())
            .collect();
        assert_eq!(slides.len(), 2);
        assert!(slides.iter().all(|slide| slide.decoration));
    }
}
