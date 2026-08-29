use serde_json::Value;

use crate::fraction::Fraction;
use crate::meta::Meta;
use crate::notes::directional::Directional;
use crate::notes::event::Event;
use crate::notes::slide::Slide;
use crate::notes::tap::Tap;
use crate::notes::{NO_NOTE, NoteBase, NoteData};
use crate::score::Score;

/// Rebase transformation: applies custom timing/BPM adjustments to a score
pub struct Rebase {
    pub offset: f64,
    pub events: Vec<Event>,
    pub meta: Meta,
}

impl Rebase {
    /// Load from a JSON string
    pub fn from_json(json_str: &str) -> Result<Rebase, serde_json::Error> {
        let v: Value = serde_json::from_str(json_str)?;
        Ok(Self::from_value(&v))
    }

    /// Load from a serde_json::Value
    pub fn from_value(v: &Value) -> Rebase {
        let offset = v.get("offset").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let events = v
            .get("events")
            .and_then(|v| v.as_array())
            .map(|events| events.iter().map(rebase_event_from_value).collect())
            .unwrap_or_default();

        Rebase {
            offset,
            events,
            meta: rebase_meta_from_value(v),
        }
    }

    /// Apply rebase transformation to a score, producing a new score
    pub fn apply(&self, source: &mut Score) -> Score {
        let mut score = Score::new();
        score.meta = source.meta.merge(&self.meta);
        score.events = self.events.clone();

        // Clone out source data to avoid borrow issues
        let active_notes = source.active_notes.clone();
        let notes_snapshot = source.notes.clone();

        let bar_to_time = collect_source_times(source, &active_notes, &notes_snapshot);

        // Rebase each note
        for &note_idx in &active_notes {
            push_rebased_note(
                &notes_snapshot[note_idx],
                &notes_snapshot,
                &bar_to_time,
                self.offset,
                &mut score,
            );
        }

        push_rebased_source_events(source, self.offset, &mut score);
        score.events.sort_by(|a, b| {
            a.bar
                .partial_cmp(&b.bar)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Sort notes and re-link
        score.notes.sort_by(|a, b| {
            a.bar()
                .partial_cmp(&b.bar())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        score.init_notes();
        score.init_events();

        score
    }
}

fn rebase_event_from_value(value: &Value) -> Event {
    let mut event = Event::new(Fraction::from_f64(
        value.get("bar").and_then(Value::as_f64).unwrap_or(0.0),
    ));
    event.bpm = value
        .get("bpm")
        .and_then(Value::as_f64)
        .map(Fraction::from_f64);
    event.bar_length = value
        .get("barLength")
        .and_then(Value::as_f64)
        .map(Fraction::from_f64);
    event.sentence_length = value
        .get("sentenceLength")
        .and_then(Value::as_i64)
        .map(|length| length as i32);
    event.section = value
        .get("section")
        .and_then(Value::as_str)
        .map(str::to_owned);
    event.text = value.get("text").and_then(Value::as_str).map(str::to_owned);
    event
}

fn rebase_meta_from_value(value: &Value) -> Meta {
    let mut meta = Meta::new();
    let Some(fields) = value.get("meta").and_then(Value::as_object) else {
        return meta;
    };
    for (name, value) in fields {
        if let Some(text) = value.as_str() {
            meta.set_field(name, text);
        } else if let Some(number) = value.as_f64() {
            meta.set_field(name, &number.to_string());
        }
    }
    meta
}

fn collect_source_times(
    source: &mut Score,
    active_notes: &[usize],
    notes: &[NoteData],
) -> std::collections::HashMap<Fraction, Fraction> {
    let mut times = std::collections::HashMap::new();
    for &note_idx in active_notes {
        for bar in referenced_note_bars(&notes[note_idx], notes) {
            times.entry(bar).or_insert_with(|| source.get_time(bar));
        }
    }
    times
}

fn referenced_note_bars(note: &NoteData, notes: &[NoteData]) -> Vec<Fraction> {
    let mut bars = vec![note.bar()];
    match note {
        NoteData::Tap(..) => {}
        NoteData::Directional(_, directional) => {
            push_indexed_bar(&mut bars, directional.tap_idx, notes);
        }
        NoteData::Slide(_, slide) => append_slide_bars(&mut bars, slide, notes),
    }
    bars
}

fn append_slide_bars(bars: &mut Vec<Fraction>, slide: &Slide, notes: &[NoteData]) {
    push_indexed_bar(bars, slide.tap_idx, notes);
    push_indexed_bar(bars, slide.directional_idx, notes);
    let Some(directional) = indexed_directional(slide.directional_idx, notes) else {
        return;
    };
    if directional.tap_idx != slide.tap_idx {
        push_indexed_bar(bars, directional.tap_idx, notes);
    }
}

fn push_indexed_bar(bars: &mut Vec<Fraction>, note_idx: usize, notes: &[NoteData]) {
    if note_idx != NO_NOTE {
        bars.push(notes[note_idx].bar());
    }
}

fn indexed_directional(note_idx: usize, notes: &[NoteData]) -> Option<&Directional> {
    (note_idx != NO_NOTE)
        .then(|| notes[note_idx].as_directional())
        .flatten()
}

fn rebase_bar(
    bar: Fraction,
    bar_to_time: &std::collections::HashMap<Fraction, Fraction>,
    offset: f64,
    score: &mut Score,
) -> Fraction {
    let source_time = bar_to_time
        .get(&bar)
        .copied()
        .unwrap_or_else(Fraction::zero)
        .to_f64();
    score.get_bar_by_time(source_time - offset)
}

fn rebased_base(
    base: &NoteBase,
    bar_to_time: &std::collections::HashMap<Fraction, Fraction>,
    offset: f64,
    score: &mut Score,
) -> NoteBase {
    NoteBase::new(
        rebase_bar(base.bar, bar_to_time, offset, score),
        base.lane,
        base.width,
        base.note_type,
    )
}

fn push_rebased_note(
    note: &NoteData,
    notes: &[NoteData],
    bar_to_time: &std::collections::HashMap<Fraction, Fraction>,
    offset: f64,
    score: &mut Score,
) {
    match note {
        NoteData::Tap(base, _) => push_rebased_tap(base, bar_to_time, offset, score),
        NoteData::Directional(base, directional) => {
            push_rebased_directional(base, bar_to_time, offset, score);
            push_rebased_tap_index(directional.tap_idx, notes, bar_to_time, offset, score);
        }
        NoteData::Slide(base, slide) => {
            push_rebased_slide(base, slide, bar_to_time, offset, score);
            push_slide_attachments(slide, notes, bar_to_time, offset, score);
        }
    }
}

fn push_rebased_tap(
    base: &NoteBase,
    bar_to_time: &std::collections::HashMap<Fraction, Fraction>,
    offset: f64,
    score: &mut Score,
) {
    let base = rebased_base(base, bar_to_time, offset, score);
    score.notes.push(NoteData::Tap(base, Tap));
}

fn push_rebased_tap_index(
    note_idx: usize,
    notes: &[NoteData],
    bar_to_time: &std::collections::HashMap<Fraction, Fraction>,
    offset: f64,
    score: &mut Score,
) {
    if note_idx != NO_NOTE {
        push_rebased_tap(notes[note_idx].base(), bar_to_time, offset, score);
    }
}

fn push_rebased_directional(
    base: &NoteBase,
    bar_to_time: &std::collections::HashMap<Fraction, Fraction>,
    offset: f64,
    score: &mut Score,
) {
    let base = rebased_base(base, bar_to_time, offset, score);
    score
        .notes
        .push(NoteData::Directional(base, Directional::new()));
}

fn push_rebased_slide(
    base: &NoteBase,
    slide: &Slide,
    bar_to_time: &std::collections::HashMap<Fraction, Fraction>,
    offset: f64,
    score: &mut Score,
) {
    let base = rebased_base(base, bar_to_time, offset, score);
    score.notes.push(NoteData::Slide(
        base,
        Slide::new(slide.channel, slide.decoration),
    ));
}

fn push_slide_attachments(
    slide: &Slide,
    notes: &[NoteData],
    bar_to_time: &std::collections::HashMap<Fraction, Fraction>,
    offset: f64,
    score: &mut Score,
) {
    push_rebased_tap_index(slide.tap_idx, notes, bar_to_time, offset, score);
    let Some(directional) = indexed_directional(slide.directional_idx, notes) else {
        return;
    };
    push_rebased_directional(
        notes[slide.directional_idx].base(),
        bar_to_time,
        offset,
        score,
    );
    if directional.tap_idx != slide.tap_idx {
        push_rebased_tap_index(directional.tap_idx, notes, bar_to_time, offset, score);
    }
}

fn push_rebased_source_events(source: &mut Score, offset: f64, score: &mut Score) {
    let mut rebased_events = Vec::new();
    for event in source.events.clone() {
        if event.speed.is_none() && event.text.is_none() {
            continue;
        }
        let source_time = source.get_time(event.bar).to_f64();
        let mut event = event;
        event.bar = score.get_bar_by_time(source_time - offset);
        rebased_events.push(event);
    }
    score.events.extend(rebased_events);
}
