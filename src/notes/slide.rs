use super::tap::Tap;
use super::{NO_NOTE, NoteData, NoteIdx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SlideType {
    Start = 1,
    End = 2,
    Relay = 3,
    Invisible = 5,
}

impl SlideType {
    pub fn from_i32(v: i32) -> Option<SlideType> {
        match v {
            1 => Some(SlideType::Start),
            2 => Some(SlideType::End),
            3 => Some(SlideType::Relay),
            5 => Some(SlideType::Invisible),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Slide {
    pub channel: i32,
    pub decoration: bool,
    pub tap_idx: NoteIdx,
    pub directional_idx: NoteIdx,
    pub next_idx: NoteIdx,
    pub head_idx: NoteIdx,
}

impl Slide {
    pub fn new(channel: i32, decoration: bool) -> Self {
        Slide {
            channel,
            decoration,
            tap_idx: NO_NOTE,
            directional_idx: NO_NOTE,
            next_idx: NO_NOTE,
            head_idx: NO_NOTE,
        }
    }

    pub fn is_path(&self, note_type: i32) -> bool {
        if note_type == 0 {
            return false;
        }
        if note_type != 3 {
            return true;
        }
        // type == 3 (RELAY)
        if self.directional_idx != NO_NOTE {
            return true;
        }
        if self.tap_idx == NO_NOTE && self.directional_idx == NO_NOTE {
            return true;
        }
        false
    }

    pub fn is_critical(&self, arena: &[NoteData]) -> bool {
        if self.tap_idx != NO_NOTE && Tap::is_critical_type(arena[self.tap_idx].note_type()) {
            return true;
        }
        if self.directional_idx != NO_NOTE
            && let Some(d) = arena[self.directional_idx].as_directional()
            && d.is_critical(arena)
        {
            return true;
        }
        if self.head_idx != NO_NOTE {
            let head = &arena[self.head_idx];
            if let Some(hs) = head.as_slide() {
                if hs.tap_idx != NO_NOTE && Tap::is_critical_type(arena[hs.tap_idx].note_type()) {
                    return true;
                }
                if hs.directional_idx != NO_NOTE
                    && let Some(d) = arena[hs.directional_idx].as_directional()
                    && d.is_critical(arena)
                {
                    return true;
                }
            }
        }
        false
    }

    pub fn is_trend(&self, arena: &[NoteData]) -> bool {
        if self.tap_idx != NO_NOTE && Tap::is_trend_type(arena[self.tap_idx].note_type()) {
            return true;
        }
        if self.directional_idx != NO_NOTE
            && let Some(d) = arena[self.directional_idx].as_directional()
            && d.is_trend(arena)
        {
            return true;
        }
        false
    }

    pub fn is_none(&self, arena: &[NoteData]) -> bool {
        if self.tap_idx != NO_NOTE && Tap::is_none_type(arena[self.tap_idx].note_type()) {
            return true;
        }
        if self.directional_idx != NO_NOTE {
            // Directional.is_none() is always false in Python
            return false;
        }
        false
    }

    pub fn is_tick(&self, arena: &[NoteData], note_type: i32) -> Option<bool> {
        if self.is_none(arena) {
            return None;
        }
        if self.decoration {
            return self.decoration_tick(arena);
        }

        if matches!(SlideType::from_i32(note_type), Some(SlideType::Invisible)) {
            return None;
        }
        if self.is_trend(arena) {
            return Some(false);
        }
        if matches!(SlideType::from_i32(note_type), Some(SlideType::Relay)) {
            return Some(false);
        }

        Some(true)
    }

    fn decoration_tick(&self, arena: &[NoteData]) -> Option<bool> {
        let tap_tick = self.attached_tap_tick(arena);
        let directional_tick = self.attached_directional_tick(arena);
        if tap_tick == Some(true) || directional_tick == Some(true) {
            return Some(true);
        }
        (self.tap_idx != NO_NOTE || self.directional_idx != NO_NOTE).then_some(false)
    }

    fn attached_tap_tick(&self, arena: &[NoteData]) -> Option<bool> {
        (self.tap_idx != NO_NOTE)
            .then(|| Tap::is_tick_with_type(&Tap, arena[self.tap_idx].note_type()))
            .flatten()
    }

    fn attached_directional_tick(&self, arena: &[NoteData]) -> Option<bool> {
        (self.directional_idx != NO_NOTE)
            .then(|| arena[self.directional_idx].as_directional())
            .flatten()
            .and_then(|directional| directional.is_tick(arena))
    }
}
