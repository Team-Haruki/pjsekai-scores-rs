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
use crate::notes::{NoteBase, NoteData};
use crate::score::Score;

const TICKS_PER_BEAT: i64 = 480;

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
    ticks: i64,
    bar: Fraction,
    bar_length: Fraction,
}

struct TickConverter {
    segments: Vec<TickSegment>,
}

impl TickConverter {
    fn new(events: &[Value]) -> Self {
        let mut converter = TickConverter {
            segments: vec![TickSegment {
                ticks: 0,
                bar: Fraction::zero(),
                bar_length: Fraction::from_integer(4),
            }],
        };

        let mut sorted: Vec<&Value> = events.iter().collect();
        sorted.sort_by_key(|event| {
            (
                i64_field(event, "ticks").unwrap_or(0),
                i64_field(event, "id").unwrap_or(0),
            )
        });

        for event in sorted {
            if i64_field(event, "eventType") != Some(3) {
                continue;
            }

            let ticks = i64_field(event, "ticks").unwrap_or(0);
            let bar_length = parse_bar_length(event.get("changeValue"));
            let bar = converter.to_bar(ticks);

            if let Some(last) = converter.segments.last_mut()
                && last.ticks == ticks
            {
                *last = TickSegment {
                    ticks,
                    bar,
                    bar_length,
                };
                continue;
            }

            converter.segments.push(TickSegment {
                ticks,
                bar,
                bar_length,
            });
        }

        converter
    }

    fn to_bar(&self, ticks: i64) -> Fraction {
        let index = self
            .segments
            .partition_point(|segment| segment.ticks <= ticks)
            .saturating_sub(1);
        let segment = self.segments[index];
        let ticks_per_bar = Fraction::from_integer(TICKS_PER_BEAT) * segment.bar_length;
        segment.bar + Fraction::from_integer(ticks - segment.ticks) / ticks_per_bar
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
    let tick_converter = TickConverter::new(event_data);

    let mut score = Score::new();
    score.meta = parse_meta(data);
    init_events_by_data(&mut score, event_data, &tick_converter);
    merge_events_by_bar(&mut score);
    init_notes_by_data(&mut score, note_data, &tick_converter);
    score.init_notes();
    score.init_events();

    Ok(score)
}

fn init_events_by_data(score: &mut Score, events: &[Value], tick_converter: &TickConverter) {
    for event in events {
        let bar = tick_converter.to_bar(i64_field(event, "ticks").unwrap_or(0));
        match i64_field(event, "eventType") {
            Some(0) => {
                if let Some(bpm) = fraction_value(event.get("changeValue")) {
                    score.events.push(Event::new(bar).with_bpm(bpm));
                }
            }
            Some(3) => {
                score.events.push(
                    Event::new(bar).with_bar_length(parse_bar_length(event.get("changeValue"))),
                );
            }
            Some(1) | Some(2) => {}
            _ => {}
        }
    }
}

fn init_notes_by_data(score: &mut Score, notes: &[Value], tick_converter: &TickConverter) {
    let mut notes_by_id: HashMap<i64, &Value> = HashMap::new();
    for note in notes {
        if let Some(id) = i64_field(note, "id") {
            notes_by_id.insert(id, note);
        }
    }

    let mut sorted: Vec<&Value> = notes.iter().collect();
    sorted.sort_by_key(|note| {
        (
            i64_field(note, "ticks").unwrap_or(0),
            i64_field(note, "id").unwrap_or(0),
        )
    });

    let mut handled_ids: HashSet<i64> = HashSet::new();
    let mut channel = 0;

    for note in sorted {
        let Some(id) = i64_field(note, "id") else {
            continue;
        };
        if handled_ids.contains(&id) {
            continue;
        }

        if is_chain_start(note) {
            channel += 1;
            append_chain(
                score,
                note,
                &notes_by_id,
                &mut handled_ids,
                tick_converter,
                channel,
            );
        } else {
            handled_ids.insert(id);
            append_note(score, note, tick_converter, 0);
        }
    }
}

fn append_chain(
    score: &mut Score,
    first: &Value,
    notes_by_id: &HashMap<i64, &Value>,
    handled_ids: &mut HashSet<i64>,
    tick_converter: &TickConverter,
    channel: i32,
) {
    let mut seen: HashSet<i64> = HashSet::new();
    let mut note = Some(first);

    while let Some(current) = note {
        let Some(id) = i64_field(current, "id") else {
            break;
        };
        if !seen.insert(id) {
            break;
        }

        handled_ids.insert(id);
        append_note(score, current, tick_converter, channel);

        let next_id = i64_field(current, "nextConnectionId").unwrap_or(-1);
        note = if next_id == -1 {
            None
        } else {
            notes_by_id.get(&next_id).copied()
        };
    }
}

fn append_note(score: &mut Score, note: &Value, tick_converter: &TickConverter, channel: i32) {
    let bar = tick_converter.to_bar(i64_field(note, "ticks").unwrap_or(0));
    let lane = lane(note);
    let width = width(note);
    let critical = i64_field(note, "type") == Some(1);
    let category = i64_field(note, "category");

    if channel != 0 {
        score.notes.extend(make_slide_notes(
            note,
            bar,
            lane,
            width,
            critical,
            channel,
            is_decoration_slide(note),
        ));
    } else {
        match category {
            Some(0) => {
                score
                    .notes
                    .push(tap_note(bar, lane, width, tap_type(critical, false, false)))
            }
            Some(3) => append_directional(score, bar, lane, width, critical, false, note),
            Some(4) => {
                score
                    .notes
                    .push(tap_note(bar, lane, width, tap_type(critical, true, false)))
            }
            Some(8) => append_directional(score, bar, lane, width, critical, true, note),
            _ => {}
        }
    }
}

fn append_directional(
    score: &mut Score,
    bar: Fraction,
    lane: i32,
    width: i32,
    critical: bool,
    trend: bool,
    note: &Value,
) {
    score
        .notes
        .push(tap_note(bar, lane, width, tap_type(critical, trend, false)));
    score.notes.push(directional_note(
        bar,
        lane,
        width,
        directional_type(i64_field(note, "direction").unwrap_or(0)),
    ));
}

fn make_slide_notes(
    note: &Value,
    bar: Fraction,
    lane: i32,
    width: i32,
    critical: bool,
    channel: i32,
    decoration: bool,
) -> Vec<NoteData> {
    let mut notes = endpoint_notes(note, bar, lane, width, critical);

    if let Some(line_directional) = line_directional(note, bar, lane, width) {
        notes.push(line_directional);
    }

    let slide = slide_note(bar, lane, width, slide_type(note), channel, decoration);

    if slide_type(note) == SlideType::Relay as i32 && bool_field(note, "isSkip") {
        if !notes.iter().any(NoteData::is_tap) {
            notes.push(tap_note(bar, lane, width, tap_type(critical, false, false)));
        }
        notes.push(slide);
        return notes;
    }

    notes.push(slide);
    notes
}

fn endpoint_notes(
    note: &Value,
    bar: Fraction,
    lane: i32,
    width: i32,
    critical: bool,
) -> Vec<NoteData> {
    let category = i64_field(note, "category");
    let note_base_type = i64_field(note, "noteBaseType");

    match note_base_type {
        Some(1) | Some(2) => {
            return vec![tap_note(bar, lane, width, tap_type(critical, false, false))];
        }
        Some(3) => {
            return vec![
                tap_note(bar, lane, width, tap_type(critical, false, false)),
                directional_note(
                    bar,
                    lane,
                    width,
                    directional_type(i64_field(note, "direction").unwrap_or(0)),
                ),
            ];
        }
        Some(9) | Some(11) => {
            return vec![tap_note(bar, lane, width, tap_type(critical, true, false))];
        }
        Some(4) => {
            return vec![
                tap_note(bar, lane, width, tap_type(critical, true, false)),
                directional_note(
                    bar,
                    lane,
                    width,
                    directional_type(i64_field(note, "direction").unwrap_or(0)),
                ),
            ];
        }
        Some(12) => {
            return vec![tap_note(bar, lane, width, tap_type(critical, false, true))];
        }
        Some(5) | Some(6) | Some(10) | Some(13) | Some(14) => return Vec::new(),
        _ => {}
    }

    match category {
        Some(0) | Some(1) => vec![tap_note(bar, lane, width, tap_type(critical, false, false))],
        Some(3) | Some(8) => {
            let trend = category == Some(8);
            vec![
                tap_note(bar, lane, width, tap_type(critical, trend, false)),
                directional_note(
                    bar,
                    lane,
                    width,
                    directional_type(i64_field(note, "direction").unwrap_or(0)),
                ),
            ]
        }
        Some(4) | Some(7) => vec![tap_note(bar, lane, width, tap_type(critical, true, false))],
        Some(5) => vec![tap_note(bar, lane, width, tap_type(critical, false, true))],
        _ => Vec::new(),
    }
}

fn parse_meta(data: &Value) -> Meta {
    let mut meta = Meta::new();
    meta.title = custom_info_string(data, "title").or_else(|| string_field(data, "title"));
    meta.difficulty = string_field(data, "musicDifficultyType");
    meta.playlevel = string_field(data, "playLevel");
    meta.songid = string_field(data, "musicId");
    meta
}

fn merge_events_by_bar(score: &mut Score) {
    let mut merged: BTreeMap<Fraction, Event> = BTreeMap::new();
    let mut events = std::mem::take(&mut score.events);
    events.sort_by(|a, b| {
        a.bar
            .partial_cmp(&b.bar)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for event in events {
        merged
            .entry(event.bar)
            .and_modify(|existing| *existing = existing.merge(&event))
            .or_insert(event);
    }

    score.events = merged.into_values().collect();
}

fn parse_bar_length(value: Option<&Value>) -> Fraction {
    if let Some(Value::String(s)) = value
        && let Some((numerator, denominator)) = s.split_once('/')
        && let (Ok(numerator), Ok(denominator)) = (
            numerator.trim().parse::<i64>(),
            denominator.trim().parse::<i64>(),
        )
    {
        return Fraction::new(numerator * 4, denominator);
    }

    fraction_value(value).unwrap_or_else(|| Fraction::from_integer(4))
}

fn fraction_value(value: Option<&Value>) -> Option<Fraction> {
    match value? {
        Value::Number(n) => n
            .as_i64()
            .map(Fraction::from_integer)
            .or_else(|| n.as_f64().map(Fraction::from_f64)),
        Value::String(s) => Fraction::parse(s),
        _ => None,
    }
}

fn lane(note: &Value) -> i32 {
    i64_field(note, "laneStart").unwrap_or(0) as i32 + 2
}

fn width(note: &Value) -> i32 {
    let start = i64_field(note, "laneStart").unwrap_or(0);
    let end = i64_field(note, "laneEnd").unwrap_or(0);
    (end - start + 1).max(1) as i32
}

fn is_chain_start(note: &Value) -> bool {
    i64_field(note, "previousConnectionId") == Some(-1)
        && i64_field(note, "nextConnectionId").unwrap_or(-1) != -1
}

fn is_decoration_slide(note: &Value) -> bool {
    matches!(i64_field(note, "category"), Some(9..=11))
        || matches!(i64_field(note, "noteBaseType"), Some(10 | 13 | 14))
}

fn slide_type(note: &Value) -> i32 {
    if bool_field(note, "IsConnectedFirst") || bool_field(note, "isConnectedFirst") {
        return SlideType::Start as i32;
    }

    if bool_field(note, "IsConnectedLast") || bool_field(note, "isConnectedLast") {
        return SlideType::End as i32;
    }

    if i64_field(note, "category") == Some(13) {
        return SlideType::Invisible as i32;
    }

    SlideType::Relay as i32
}

fn tap_type(critical: bool, trend: bool, cancel: bool) -> i32 {
    match (critical, trend, cancel) {
        (true, true, false) => TapType::CriticalTrend as i32,
        (false, true, false) => TapType::Trend as i32,
        (true, false, true) => TapType::CriticalCancel as i32,
        (false, false, true) => TapType::Cancel as i32,
        (true, false, false) => TapType::Critical as i32,
        (false, false, false) => TapType::Tap as i32,
        (_, true, true) => TapType::Cancel as i32,
    }
}

fn directional_type(direction: i64) -> i32 {
    match direction {
        1 => DirectionalType::UpperLeft as i32,
        2 => DirectionalType::UpperRight as i32,
        _ => DirectionalType::Up as i32,
    }
}

fn line_directional(note: &Value, bar: Fraction, lane: i32, width: i32) -> Option<NoteData> {
    let note_type = match i64_field(note, "noteLineType").unwrap_or(0) {
        1 => DirectionalType::LowerLeft as i32,
        2 => DirectionalType::Down as i32,
        _ => return None,
    };

    Some(directional_note(bar, lane, width, note_type))
}

fn tap_note(bar: Fraction, lane: i32, width: i32, note_type: i32) -> NoteData {
    NoteData::Tap(NoteBase::new(bar, lane, width, note_type), Tap)
}

fn directional_note(bar: Fraction, lane: i32, width: i32, note_type: i32) -> NoteData {
    NoteData::Directional(
        NoteBase::new(bar, lane, width, note_type),
        Directional::new(),
    )
}

fn slide_note(
    bar: Fraction,
    lane: i32,
    width: i32,
    note_type: i32,
    channel: i32,
    decoration: bool,
) -> NoteData {
    NoteData::Slide(
        NoteBase::new(bar, lane, width, note_type),
        Slide::new(channel, decoration),
    )
}

fn array_field<'a>(object: &'a Map<String, Value>, key: &str) -> &'a [Value] {
    object
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn i64_field(value: &Value, key: &str) -> Option<i64> {
    match value.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn bool_field(value: &Value, key: &str) -> bool {
    match value.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(n)) => n.as_i64().is_some_and(|v| v != 0),
        Some(Value::String(s)) => matches!(s.as_str(), "true" | "True" | "1"),
        _ => false,
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
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
              {"id": 1, "ticks": 0, "laneStart": 0, "laneEnd": 1, "category": 0, "type": 0},
              {"id": 2, "ticks": 480, "laneStart": 2, "laneEnd": 3, "category": 3, "type": 1, "direction": 2}
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
    fn parses_connected_slide_chain() {
        let json = r#"
        {
          "MusicScoreEventDataList": [
            {"id": 1, "ticks": 0, "eventType": 0, "changeValue": 120}
          ],
          "NoteList": [
            {"id": 10, "ticks": 0, "laneStart": 0, "laneEnd": 1, "category": 1, "type": 0, "previousConnectionId": -1, "nextConnectionId": 11, "IsConnectedFirst": true, "noteBaseType": 1},
            {"id": 11, "ticks": 480, "laneStart": 1, "laneEnd": 2, "category": 1, "type": 0, "previousConnectionId": 10, "nextConnectionId": 12, "noteBaseType": 6},
            {"id": 12, "ticks": 960, "laneStart": 2, "laneEnd": 3, "category": 1, "type": 0, "previousConnectionId": 11, "nextConnectionId": -1, "IsConnectedLast": true, "noteBaseType": 2}
          ]
        }
        "#;

        let score = parse_score_json(json).unwrap();
        let slide_count = score
            .active_notes
            .iter()
            .filter(|&&idx| score.notes[idx].is_slide())
            .count();
        assert_eq!(slide_count, 3);
    }
}
