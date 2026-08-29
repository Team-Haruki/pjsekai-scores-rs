use regex::Regex;
use std::sync::LazyLock;

use crate::fraction::Fraction;
use crate::meta::Meta;
use crate::notes::directional::Directional;
use crate::notes::event::Event;
use crate::notes::slide::Slide;
use crate::notes::tap::Tap;
use crate::notes::{NoteBase, NoteData};

// Pre-compiled regex patterns
static RE_META: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#(\w+)\s+(.*)$").unwrap());
static RE_SCORE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#(\w+):\s*(.*)$").unwrap());
static RE_EVENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{3})02$").unwrap());
static RE_BPM_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^BPM(..)$").unwrap());
static RE_BPM_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{3})08$").unwrap());
static RE_TIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^TIL(..)$").unwrap());
static RE_SPEED_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+)'(\d+):([^\s,]+)").unwrap());
static RE_TAP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{3})1(.)$").unwrap());
static RE_SLIDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{3})3(.)(.)$").unwrap());
static RE_DIRECTIONAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{3})5(.)$").unwrap());
static RE_DECO_SLIDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{3})9(.)(.)$").unwrap());
static RE_TICKS_PER_BEAT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^"ticks_per_beat\s+(\d+)"$"#).unwrap());

/// BPM definition from the score file
#[derive(Debug, Clone)]
pub struct BpmDefinition {
    pub id: i32,
    pub bpm: Fraction,
}

/// BPM reference at a bar position
#[derive(Debug, Clone)]
pub struct BpmReference {
    pub bar: Fraction,
    pub id: i32,
}

/// Speed definition item
#[derive(Debug, Clone)]
pub struct SpeedDefinitionItem {
    pub bar: i32,
    pub tick: i32,
    pub speed: f64,
}

/// Speed definition
#[derive(Debug, Clone)]
pub struct SpeedDefinition {
    pub id: i32,
    pub items: Vec<SpeedDefinitionItem>,
}

/// Speed control toggle
#[derive(Debug, Clone)]
pub struct SpeedControl {
    pub id: Option<i32>,
}

/// Ticks per beat (usually 480)
#[derive(Debug, Clone, Copy)]
pub struct TicksPerBeat(pub i32);

impl Default for TicksPerBeat {
    fn default() -> Self {
        TicksPerBeat(480)
    }
}

/// Items that can be parsed from a line
pub enum ParsedItem {
    Meta(Box<Meta>),
    TicksPerBeat(TicksPerBeat),
    SpeedControl(SpeedControl),
    SpeedDefinition(SpeedDefinition),
    Event(Event),
    BpmDefinition(BpmDefinition),
    BpmReference(BpmReference),
    Note(NoteData),
}

/// Parse base36 character to integer
fn base36_char(c: char) -> i32 {
    if c.is_ascii_digit() {
        (c as i32) - ('0' as i32)
    } else if c.is_ascii_uppercase() {
        (c as i32) - ('A' as i32) + 10
    } else if c.is_ascii_lowercase() {
        (c as i32) - ('a' as i32) + 10
    } else {
        0
    }
}

/// Parse a two-character base36 string to integer
fn base36_two(s: &str) -> i32 {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() >= 2 {
        base36_char(chars[0]) * 36 + base36_char(chars[1])
    } else if chars.len() == 1 {
        base36_char(chars[0])
    } else {
        0
    }
}

/// Parse score data into (beat_fraction, two-char data) pairs
fn parse_score_data(data: &str) -> Vec<(Fraction, String)> {
    let mut results = Vec::new();
    let chars: Vec<char> = data.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i + 1 < len {
        let pair: String = chars[i..i + 2].iter().collect();
        if pair != "00" {
            results.push((Fraction::new(i as i64, len as i64), pair));
        }
        i += 2;
    }
    results
}

/// Strip surrounding quotes from a string (matching Python's eval for simple strings)
fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// A parsed line from a .sus file
pub struct Line {
    pub line_type: LineType,
    pub header: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LineType {
    Meta,
    Score,
    Comment,
}

impl Line {
    pub fn new(raw: &str) -> Self {
        let line = raw.trim();

        if let Some(caps) = RE_META.captures(line) {
            Line {
                line_type: LineType::Meta,
                header: caps[1].to_string(),
                data: caps[2].to_string(),
            }
        } else if let Some(caps) = RE_SCORE.captures(line) {
            Line {
                line_type: LineType::Score,
                header: caps[1].to_string(),
                data: caps[2].to_string(),
            }
        } else {
            Line {
                line_type: LineType::Comment,
                header: "comment".to_string(),
                data: line.to_string(),
            }
        }
    }

    /// Parse this line into zero or more items
    pub fn parse(&self) -> Vec<ParsedItem> {
        match self.line_type {
            LineType::Meta => self.parse_meta(),
            LineType::Score => self.parse_score(),
            LineType::Comment => Vec::new(),
        }
    }

    fn parse_meta(&self) -> Vec<ParsedItem> {
        let field_name = self.header.to_lowercase();
        if Meta::has_field(&field_name) {
            let data = strip_quotes(&self.data);
            let mut meta = Meta::new();
            meta.set_field(&field_name, &data);
            return vec![ParsedItem::Meta(Box::new(meta))];
        }

        if self.header == "REQUEST" {
            if let Some(caps) = RE_TICKS_PER_BEAT.captures(&self.data)
                && let Ok(tpb) = caps[1].parse::<i32>()
            {
                return vec![ParsedItem::TicksPerBeat(TicksPerBeat(tpb))];
            }
        } else if self.header == "HISPEED" {
            let id = i32::from_str_radix(&self.data, 36).unwrap_or(0);
            return vec![ParsedItem::SpeedControl(SpeedControl { id: Some(id) })];
        } else if self.header == "NOSPEED" {
            return vec![ParsedItem::SpeedControl(SpeedControl { id: None })];
        }

        Vec::new()
    }

    fn parse_score(&self) -> Vec<ParsedItem> {
        if let Some(caps) = RE_EVENT.captures(&self.header) {
            let bar: i64 = caps[1].parse().unwrap_or(0);
            let bar_length: i64 = self.data.trim().parse().unwrap_or(4);
            return vec![ParsedItem::Event(
                Event::new(Fraction::from_integer(bar))
                    .with_bar_length(Fraction::from_integer(bar_length)),
            )];
        }

        if let Some(caps) = RE_BPM_DEF.captures(&self.header) {
            let id = base36_two(&caps[1]);
            if let Some(bpm) = Fraction::parse(&self.data) {
                return vec![ParsedItem::BpmDefinition(BpmDefinition { id, bpm })];
            }
        }

        if let Some(caps) = RE_BPM_REF.captures(&self.header) {
            let bar_num: i64 = caps[1].parse().unwrap_or(0);
            return parse_bpm_references(bar_num, &self.data);
        }

        if let Some(caps) = RE_TIL.captures(&self.header) {
            let id = base36_two(&caps[1]);
            return vec![ParsedItem::SpeedDefinition(parse_speed_definition(
                id, &self.data,
            ))];
        }

        if let Some(caps) = RE_TAP.captures(&self.header) {
            let bar_num: i64 = caps[1].parse().unwrap_or(0);
            let lane = base36_char(caps[2].chars().next().unwrap_or('0'));
            return parse_note_items(bar_num, lane, &self.data, ScoreNoteKind::Tap);
        }

        if let Some(caps) = RE_SLIDE.captures(&self.header) {
            let bar_num: i64 = caps[1].parse().unwrap_or(0);
            let lane = base36_char(caps[2].chars().next().unwrap_or('0'));
            let channel = base36_char(caps[3].chars().next().unwrap_or('0'));
            return parse_note_items(
                bar_num,
                lane,
                &self.data,
                ScoreNoteKind::Slide {
                    channel,
                    decoration: false,
                },
            );
        }

        if let Some(caps) = RE_DIRECTIONAL.captures(&self.header) {
            let bar_num: i64 = caps[1].parse().unwrap_or(0);
            let lane = base36_char(caps[2].chars().next().unwrap_or('0'));
            return parse_note_items(bar_num, lane, &self.data, ScoreNoteKind::Directional);
        }

        if let Some(caps) = RE_DECO_SLIDE.captures(&self.header) {
            let bar_num: i64 = caps[1].parse().unwrap_or(0);
            let lane = base36_char(caps[2].chars().next().unwrap_or('0'));
            let channel = base36_char(caps[3].chars().next().unwrap_or('0'));
            return parse_note_items(
                bar_num,
                lane,
                &self.data,
                ScoreNoteKind::Slide {
                    channel,
                    decoration: true,
                },
            );
        }

        Vec::new()
    }
}

#[derive(Clone, Copy)]
enum ScoreNoteKind {
    Tap,
    Slide { channel: i32, decoration: bool },
    Directional,
}

fn parse_bpm_references(bar_num: i64, data: &str) -> Vec<ParsedItem> {
    parse_score_data(data)
        .into_iter()
        .map(|(beat, data)| {
            ParsedItem::BpmReference(BpmReference {
                bar: Fraction::from_integer(bar_num) + beat,
                id: base36_two(&data),
            })
        })
        .collect()
}

fn parse_speed_definition(id: i32, raw_data: &str) -> SpeedDefinition {
    let data = strip_quotes(raw_data);
    let mut items = RE_SPEED_ITEM
        .captures_iter(&data)
        .map(|item_match| SpeedDefinitionItem {
            bar: item_match[1].parse().unwrap_or(0),
            tick: item_match[2].parse().unwrap_or(0),
            speed: item_match[3].parse().unwrap_or(1.0),
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| (item.bar, item.tick));
    SpeedDefinition { id, items }
}

fn parse_note_items(bar_num: i64, lane: i32, data: &str, kind: ScoreNoteKind) -> Vec<ParsedItem> {
    parse_score_data(data)
        .into_iter()
        .map(|(beat, data)| {
            let chars = data.as_bytes();
            let note_type = base36_char(chars[0] as char);
            let width = base36_char(chars[1] as char);
            let base = NoteBase::new(
                Fraction::from_integer(bar_num) + beat,
                lane,
                width,
                note_type,
            );
            let note = match kind {
                ScoreNoteKind::Tap => NoteData::Tap(base, Tap),
                ScoreNoteKind::Slide {
                    channel,
                    decoration,
                } => NoteData::Slide(base, Slide::new(channel, decoration)),
                ScoreNoteKind::Directional => NoteData::Directional(base, Directional::new()),
            };
            ParsedItem::Note(note)
        })
        .collect()
}
