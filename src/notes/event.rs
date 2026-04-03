use crate::fraction::Fraction;

/// An event in the score timeline (BPM changes, bar length, sections, etc.)
#[derive(Debug, Clone)]
pub struct Event {
    pub bar: Fraction,
    pub bpm: Option<Fraction>,
    pub bar_length: Option<Fraction>,
    pub sentence_length: Option<i32>,
    pub speed: Option<f64>,
    pub section: Option<String>,
    pub text: Option<String>,
}

impl Event {
    pub fn new(bar: Fraction) -> Self {
        Event {
            bar,
            bpm: None,
            bar_length: None,
            sentence_length: None,
            speed: None,
            section: None,
            text: None,
        }
    }

    pub fn with_bpm(mut self, bpm: Fraction) -> Self {
        self.bpm = Some(bpm);
        self
    }

    pub fn with_bar_length(mut self, bar_length: Fraction) -> Self {
        self.bar_length = Some(bar_length);
        self
    }

    pub fn with_sentence_length(mut self, sl: i32) -> Self {
        self.sentence_length = Some(sl);
        self
    }

    pub fn with_speed(mut self, speed: f64) -> Self {
        self.speed = Some(speed);
        self
    }

    pub fn with_section(mut self, section: String) -> Self {
        self.section = Some(section);
        self
    }

    pub fn with_text(mut self, text: String) -> Self {
        self.text = Some(text);
        self
    }

    /// Merge operator matching Python's `__or__`: prefer other's non-None values
    pub fn merge(&self, other: &Event) -> Event {
        debug_assert!(self.bar <= other.bar);
        Event {
            bar: other.bar,
            bpm: or_falsy_fraction(other.bpm, self.bpm),
            bar_length: or_falsy_fraction(other.bar_length, self.bar_length),
            sentence_length: or_falsy_i32(other.sentence_length, self.sentence_length),
            speed: or_falsy_f64(other.speed, self.speed),
            section: or_falsy_string(&other.section, &self.section),
            text: or_falsy_string(&other.text, &self.text),
        }
    }

    /// Merge in-place: update self with non-None values from other
    pub fn merge_from(&mut self, other: &Event) {
        let merged = self.merge(other);
        *self = merged;
    }
}

fn or_falsy_fraction(a: Option<Fraction>, b: Option<Fraction>) -> Option<Fraction> {
    match a {
        Some(v) if v != Fraction::zero() => Some(v),
        _ => b,
    }
}

fn or_falsy_i32(a: Option<i32>, b: Option<i32>) -> Option<i32> {
    match a {
        Some(v) if v != 0 => Some(v),
        _ => b,
    }
}

fn or_falsy_f64(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match a {
        Some(v) if v != 0.0 => Some(v),
        _ => b,
    }
}

fn or_falsy_string(a: &Option<String>, b: &Option<String>) -> Option<String> {
    match a {
        Some(v) if !v.is_empty() => Some(v.clone()),
        _ => b.clone(),
    }
}
