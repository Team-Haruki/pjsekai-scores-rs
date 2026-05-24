use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

use crate::drawing::{Drawing, MusicMeta};
use crate::fraction::Fraction;
use crate::lyric::Lyric;
use crate::meta::Meta;
use crate::notes::event::Event;
use crate::rebase::Rebase;
use crate::score::Score;

#[wasm_bindgen(js_name = Score)]
#[derive(Clone)]
pub struct WasmScore {
    inner: Score,
}

#[wasm_bindgen(js_class = Score)]
impl WasmScore {
    #[wasm_bindgen(js_name = fromSus)]
    pub fn from_sus(content: &str) -> WasmScore {
        WasmScore {
            inner: Score::parse(content),
        }
    }

    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(content: &str) -> Result<WasmScore, JsError> {
        let score = Score::parse_json(content)
            .map_err(|e| JsError::new(&format!("Invalid score JSON: {e}")))?;
        Ok(WasmScore { inner: score })
    }

    #[wasm_bindgen(js_name = load)]
    pub fn load(content: &str) -> Result<WasmScore, JsError> {
        let score = Score::parse_auto(content)
            .map_err(|e| JsError::new(&format!("Invalid score content: {e}")))?;
        Ok(WasmScore { inner: score })
    }

    pub fn copy(&self) -> WasmScore {
        WasmScore {
            inner: self.inner.clone(),
        }
    }

    #[wasm_bindgen(getter, js_name = noteCount)]
    pub fn note_count(&self) -> u32 {
        self.inner.active_notes.len() as u32
    }

    #[wasm_bindgen(getter, js_name = eventCount)]
    pub fn event_count(&self) -> u32 {
        self.inner.events.len() as u32
    }

    #[wasm_bindgen(getter)]
    pub fn title(&self) -> Option<String> {
        self.inner.meta.title.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn artist(&self) -> Option<String> {
        self.inner.meta.artist.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn difficulty(&self) -> Option<String> {
        self.inner.meta.difficulty.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn playlevel(&self) -> Option<String> {
        self.inner.meta.playlevel.clone()
    }

    #[wasm_bindgen(js_name = setMetaField)]
    pub fn set_meta_field(&mut self, name: &str, value: &str) -> bool {
        if Meta::has_field(name) {
            self.inner.meta.set_field(name, value);
            true
        } else {
            false
        }
    }

    #[wasm_bindgen(js_name = setMetaJson)]
    pub fn set_meta_json(&mut self, meta_json: &str) -> Result<(), JsError> {
        let value: Value = serde_json::from_str(meta_json)
            .map_err(|e| JsError::new(&format!("Invalid metadata JSON: {e}")))?;
        let object = value
            .as_object()
            .ok_or_else(|| JsError::new("Metadata JSON must be an object"))?;
        for (key, value) in object {
            if !Meta::has_field(key) {
                continue;
            }
            if let Some(s) = value.as_str() {
                self.inner.meta.set_field(key, s);
            } else if let Some(n) = value.as_f64() {
                self.inner.meta.set_field(key, &n.to_string());
            }
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = metaJson)]
    pub fn meta_json(&self) -> String {
        meta_to_json(&self.inner.meta).to_string()
    }

    #[wasm_bindgen(js_name = eventsJson)]
    pub fn events_json(&self) -> String {
        Value::Array(self.inner.events.iter().map(event_to_json).collect()).to_string()
    }

    #[wasm_bindgen(js_name = getTime)]
    pub fn get_time(&mut self, bar: f64) -> Result<f64, JsError> {
        let bar = fraction_from_number("bar", bar)?;
        Ok(self.inner.get_time(bar).to_f64())
    }

    #[wasm_bindgen(js_name = getTimeText)]
    pub fn get_time_text(&mut self, bar: f64) -> Result<String, JsError> {
        let bar = fraction_from_number("bar", bar)?;
        Ok(self.inner.get_time(bar).to_string())
    }

    #[wasm_bindgen(js_name = getTimeDelta)]
    pub fn get_time_delta(&mut self, bar_from: f64, bar_to: f64) -> Result<f64, JsError> {
        let bar_from = fraction_from_number("barFrom", bar_from)?;
        let bar_to = fraction_from_number("barTo", bar_to)?;
        Ok(self.inner.get_time_delta(bar_from, bar_to).to_f64())
    }

    #[wasm_bindgen(js_name = getBarByTime)]
    pub fn get_bar_by_time(&mut self, time: f64) -> Result<f64, JsError> {
        check_finite("time", time)?;
        Ok(self.inner.get_bar_by_time(time).to_f64())
    }

    #[wasm_bindgen(js_name = getBarByTimeText)]
    pub fn get_bar_by_time_text(&mut self, time: f64) -> Result<String, JsError> {
        check_finite("time", time)?;
        Ok(self.inner.get_bar_by_time(time).to_string())
    }

    pub fn svg(&mut self) -> String {
        let mut drawing = Drawing::new(None, None, false, None, None, None);
        drawing.svg(&mut self.inner, None)
    }
}

#[wasm_bindgen(js_name = Lyric)]
pub struct WasmLyric {
    inner: Lyric,
}

#[wasm_bindgen(js_class = Lyric)]
impl WasmLyric {
    #[wasm_bindgen(js_name = fromText)]
    pub fn from_text(content: &str) -> WasmLyric {
        WasmLyric {
            inner: Lyric::load(content),
        }
    }

    #[wasm_bindgen(getter, js_name = wordCount)]
    pub fn word_count(&self) -> u32 {
        self.inner.words.len() as u32
    }
}

#[wasm_bindgen(js_name = Rebase)]
pub struct WasmRebase {
    inner: Rebase,
}

#[wasm_bindgen(js_class = Rebase)]
impl WasmRebase {
    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(json_str: &str) -> Result<WasmRebase, JsError> {
        let rebase = Rebase::from_json(json_str)
            .map_err(|e| JsError::new(&format!("Invalid rebase JSON: {e}")))?;
        Ok(WasmRebase { inner: rebase })
    }

    pub fn apply(&self, score: &mut WasmScore) -> WasmScore {
        WasmScore {
            inner: self.inner.apply(&mut score.inner),
        }
    }
}

#[wasm_bindgen(js_name = Drawing)]
pub struct WasmDrawing {
    inner: Drawing,
}

impl Default for WasmDrawing {
    fn default() -> Self {
        WasmDrawing {
            inner: Drawing::new(None, None, false, None, None, None),
        }
    }
}

#[wasm_bindgen(js_class = Drawing)]
impl WasmDrawing {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmDrawing {
        WasmDrawing::default()
    }

    #[wasm_bindgen(js_name = setNoteHost)]
    pub fn set_note_host(&mut self, note_host: &str) {
        self.inner.config.note_host = note_host.to_string();
    }

    #[wasm_bindgen(js_name = setStyleSheet)]
    pub fn set_style_sheet(&mut self, style_sheet: Option<String>) {
        self.inner.set_style_sheet(style_sheet);
    }

    #[wasm_bindgen(js_name = setSkill)]
    pub fn set_skill(&mut self, skill: bool) {
        self.inner.skill = skill;
    }

    #[wasm_bindgen(js_name = setMusicMetaJson)]
    pub fn set_music_meta_json(&mut self, music_meta_json: &str) -> Result<(), JsError> {
        self.inner.music_meta = Some(parse_music_meta_json(music_meta_json)?);
        Ok(())
    }

    #[wasm_bindgen(js_name = clearMusicMeta)]
    pub fn clear_music_meta(&mut self) {
        self.inner.music_meta = None;
    }

    #[wasm_bindgen(js_name = setGenerator)]
    pub fn set_generator(&mut self, generator: &str) {
        self.inner.config.generator = generator.to_string();
    }

    #[wasm_bindgen(js_name = setNoteAssetExtension)]
    pub fn set_note_asset_extension(&mut self, extension: &str) {
        self.inner.set_note_asset_extension(extension);
    }

    #[wasm_bindgen(getter, js_name = noteSize)]
    pub fn note_size(&self) -> i32 {
        self.inner.config.note_size
    }

    #[wasm_bindgen(setter, js_name = noteSize)]
    pub fn set_note_size(&mut self, value: i32) {
        self.inner.config.note_size = value;
    }

    #[wasm_bindgen(getter, js_name = timeHeight)]
    pub fn time_height(&self) -> f64 {
        self.inner.config.time_height
    }

    #[wasm_bindgen(setter, js_name = timeHeight)]
    pub fn set_time_height(&mut self, value: f64) {
        self.inner.config.time_height = value;
    }

    #[wasm_bindgen(getter, js_name = laneWidth)]
    pub fn lane_width(&self) -> i32 {
        self.inner.config.lane_width
    }

    #[wasm_bindgen(setter, js_name = laneWidth)]
    pub fn set_lane_width(&mut self, value: i32) {
        self.inner.config.lane_width = value;
    }

    pub fn svg(&mut self, score: &mut WasmScore) -> String {
        self.inner.svg(&mut score.inner, None)
    }

    #[wasm_bindgen(js_name = svgWithLyric)]
    pub fn svg_with_lyric(&mut self, score: &mut WasmScore, lyric: &WasmLyric) -> String {
        self.inner.svg(&mut score.inner, Some(&lyric.inner))
    }
}

#[wasm_bindgen(js_name = susToSvg)]
pub fn sus_to_svg(content: &str) -> String {
    let mut score = Score::parse(content);
    let mut drawing = Drawing::new(None, None, false, None, None, None);
    drawing.svg(&mut score, None)
}

#[wasm_bindgen(js_name = jsonToSvg)]
pub fn json_to_svg(content: &str) -> Result<String, JsError> {
    let mut score = Score::parse_json(content)
        .map_err(|e| JsError::new(&format!("Invalid score JSON: {e}")))?;
    let mut drawing = Drawing::new(None, None, false, None, None, None);
    Ok(drawing.svg(&mut score, None))
}

#[wasm_bindgen(js_name = scoreToSvg)]
pub fn score_to_svg(content: &str) -> Result<String, JsError> {
    let mut score = Score::parse_auto(content)
        .map_err(|e| JsError::new(&format!("Invalid score content: {e}")))?;
    let mut drawing = Drawing::new(None, None, false, None, None, None);
    Ok(drawing.svg(&mut score, None))
}

fn check_finite(name: &str, value: f64) -> Result<(), JsError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(JsError::new(&format!("{name} must be a finite number")))
    }
}

fn fraction_from_number(name: &str, value: f64) -> Result<Fraction, JsError> {
    check_finite(name, value)?;
    Ok(Fraction::from_f64(value))
}

fn parse_music_meta_json(json_str: &str) -> Result<MusicMeta, JsError> {
    let value: Value = serde_json::from_str(json_str)
        .map_err(|e| JsError::new(&format!("Invalid music metadata JSON: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| JsError::new("Music metadata JSON must be an object"))?;

    Ok(MusicMeta {
        fever_end_time: optional_f64(object.get("fever_end_time"), "fever_end_time")?
            .unwrap_or(0.0),
        fever_score: optional_f64(object.get("fever_score"), "fever_score")?.unwrap_or(0.0),
        skill_score_solo: optional_f64_array(object.get("skill_score_solo"), "skill_score_solo")?,
        skill_score_multi: optional_f64_array(
            object.get("skill_score_multi"),
            "skill_score_multi",
        )?,
    })
}

fn optional_f64(value: Option<&Value>, name: &str) -> Result<Option<f64>, JsError> {
    let Some(value) = value else {
        return Ok(None);
    };
    value
        .as_f64()
        .map(Some)
        .ok_or_else(|| JsError::new(&format!("{name} must be a number")))
}

fn optional_f64_array(value: Option<&Value>, name: &str) -> Result<Vec<f64>, JsError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| JsError::new(&format!("{name} must be an array")))?;
    array
        .iter()
        .enumerate()
        .map(|(i, value)| {
            value
                .as_f64()
                .ok_or_else(|| JsError::new(&format!("{name}[{i}] must be a number")))
        })
        .collect()
}

fn meta_to_json(meta: &Meta) -> Value {
    json!({
        "title": meta.title,
        "subtitle": meta.subtitle,
        "artist": meta.artist,
        "genre": meta.genre,
        "designer": meta.designer,
        "difficulty": meta.difficulty,
        "playlevel": meta.playlevel,
        "songid": meta.songid,
        "wave": meta.wave,
        "waveoffset": meta.waveoffset,
        "jacket": meta.jacket,
        "background": meta.background,
        "movie": meta.movie,
        "movieoffset": meta.movieoffset,
        "basebpm": meta.basebpm,
    })
}

fn event_to_json(event: &Event) -> Value {
    json!({
        "bar": fraction_to_json(event.bar),
        "bpm": event.bpm.map(fraction_to_json),
        "barLength": event.bar_length.map(fraction_to_json),
        "sentenceLength": event.sentence_length,
        "speed": event.speed,
        "section": event.section,
        "text": event.text,
    })
}

fn fraction_to_json(fraction: Fraction) -> Value {
    json!({
        "value": fraction.to_f64(),
        "text": fraction.to_string(),
        "numerator": *fraction.numer(),
        "denominator": *fraction.denom(),
    })
}
