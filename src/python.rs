use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::drawing::{Drawing, MusicMeta};
use crate::lyric::Lyric;
use crate::rebase::Rebase;
use crate::score::Score;

/// Lightweight event view exposed to Python
#[pyclass(name = "Event")]
struct PyEvent {
    #[pyo3(get)]
    bar: f64,
    #[pyo3(get)]
    bpm: Option<f64>,
    #[pyo3(get)]
    speed: Option<f64>,
    #[pyo3(get)]
    text: Option<String>,
}

/// Python wrapper for Score
#[pyclass(name = "Score")]
struct PyScore {
    inner: Score,
}

#[pymethods]
impl PyScore {
    /// Open and parse a .sus file
    #[staticmethod]
    fn open(path: &str) -> PyResult<PyScore> {
        let score = Score::open(path).map_err(|e| {
            pyo3::exceptions::PyIOError::new_err(format!("Failed to open score: {e}"))
        })?;
        Ok(PyScore { inner: score })
    }

    /// Parse from string content
    #[staticmethod]
    fn from_str(content: &str) -> PyScore {
        PyScore {
            inner: content.parse().unwrap(),
        }
    }

    /// Set metadata fields (keyword args, all optional)
    #[pyo3(signature = (title=None, artist=None, difficulty=None, playlevel=None, jacket=None, songid=None, subtitle=None))]
    fn set_meta(
        &mut self,
        title: Option<String>,
        artist: Option<String>,
        difficulty: Option<String>,
        playlevel: Option<String>,
        jacket: Option<String>,
        songid: Option<String>,
        subtitle: Option<String>,
    ) {
        if let Some(v) = title      { self.inner.meta.title = Some(v); }
        if let Some(v) = artist     { self.inner.meta.artist = Some(v); }
        if let Some(v) = difficulty { self.inner.meta.difficulty = Some(v); }
        if let Some(v) = playlevel  { self.inner.meta.playlevel = Some(v); }
        if let Some(v) = jacket     { self.inner.meta.jacket = Some(v); }
        if let Some(v) = songid     { self.inner.meta.songid = Some(v); }
        if let Some(v) = subtitle   { self.inner.meta.subtitle = Some(v); }
    }

    /// Get the number of active notes
    fn note_count(&self) -> usize {
        self.inner.active_notes.len()
    }

    /// Get the number of events
    fn event_count(&self) -> usize {
        self.inner.events.len()
    }

    /// Get the title
    fn title(&self) -> Option<String> {
        self.inner.meta.title.clone()
    }

    /// Get the artist
    fn artist(&self) -> Option<String> {
        self.inner.meta.artist.clone()
    }

    /// Get the difficulty
    fn difficulty(&self) -> Option<String> {
        self.inner.meta.difficulty.clone()
    }

    /// Get the play level
    fn playlevel(&self) -> Option<String> {
        self.inner.meta.playlevel.clone()
    }

    /// Get all events as a list of Event objects
    fn events(&self) -> Vec<PyEvent> {
        self.inner.events.iter().map(|e| PyEvent {
            bar: e.bar.to_f64(),
            bpm: e.bpm.as_ref().map(|b| b.to_f64()),
            speed: e.speed,
            text: e.text.clone(),
        }).collect()
    }
}

/// Python wrapper for Lyric
#[pyclass(name = "Lyric")]
struct PyLyric {
    inner: Lyric,
}

#[pymethods]
impl PyLyric {
    /// Load lyrics from a string
    #[staticmethod]
    fn load(content: &str) -> PyLyric {
        PyLyric {
            inner: Lyric::load(content),
        }
    }

    /// Get the number of words
    fn word_count(&self) -> usize {
        self.inner.words.len()
    }
}

/// Python wrapper for Rebase
#[pyclass(name = "Rebase")]
struct PyRebase {
    inner: Rebase,
}

#[pymethods]
impl PyRebase {
    /// Load from JSON string
    #[staticmethod]
    fn from_json(json_str: &str) -> PyResult<PyRebase> {
        let rebase = Rebase::from_json(json_str).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid JSON: {e}"))
        })?;
        Ok(PyRebase { inner: rebase })
    }

    /// Load from a Python dict (serialised to JSON internally)
    #[staticmethod]
    fn from_dict(py: pyo3::Python<'_>, dict: &Bound<'_, pyo3::types::PyAny>) -> PyResult<PyRebase> {
        let json_module = py.import("json")?;
        let json_str: String = json_module.call_method1("dumps", (dict,))?.extract()?;
        let rebase = Rebase::from_json(&json_str).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid rebase dict: {e}"))
        })?;
        Ok(PyRebase { inner: rebase })
    }

    /// Apply rebase to a score
    fn apply(&self, score: &mut PyScore) -> PyScore {
        let new_score = self.inner.apply(&mut score.inner);
        PyScore { inner: new_score }
    }
}

/// Python wrapper for Drawing
#[pyclass(name = "Drawing")]
struct PyDrawing {
    inner: Drawing,
}

#[pymethods]
impl PyDrawing {
    #[new]
    #[pyo3(signature = (note_host=None, style_sheet=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None))]
    fn new(
        note_host: Option<String>,
        style_sheet: Option<String>,
        skill: bool,
        music_meta: Option<&Bound<'_, PyDict>>,
        target_segment_seconds: Option<f64>,
        generator: Option<String>,
    ) -> PyResult<PyDrawing> {
        let mm = if let Some(meta_dict) = music_meta {
            let fever_end_time: f64 = meta_dict
                .get_item("fever_end_time")?
                .map(|v| v.extract::<f64>())
                .transpose()?
                .unwrap_or(0.0);
            let fever_score: f64 = meta_dict
                .get_item("fever_score")?
                .map(|v| v.extract::<f64>())
                .transpose()?
                .unwrap_or(0.0);
            let skill_score_solo: Vec<f64> = meta_dict
                .get_item("skill_score_solo")?
                .map(|v| v.extract::<Vec<f64>>())
                .transpose()?
                .unwrap_or_default();
            let skill_score_multi: Vec<f64> = meta_dict
                .get_item("skill_score_multi")?
                .map(|v| v.extract::<Vec<f64>>())
                .transpose()?
                .unwrap_or_default();
            Some(MusicMeta {
                fever_end_time,
                fever_score,
                skill_score_solo,
                skill_score_multi,
            })
        } else {
            None
        };

        Ok(PyDrawing {
            inner: Drawing::new(note_host, style_sheet, skill, mm, target_segment_seconds, generator),
        })
    }

    /// Generate SVG string from a score
    #[pyo3(signature = (score, lyric=None))]
    fn svg(&mut self, score: &mut PyScore, lyric: Option<&PyLyric>) -> String {
        self.inner
            .svg(&mut score.inner, lyric.map(|l| &l.inner))
    }

    #[getter]
    fn note_size(&self) -> i32 { self.inner.config.note_size }
    #[setter]
    fn set_note_size(&mut self, v: i32) { self.inner.config.note_size = v; }

    #[getter]
    fn time_height(&self) -> f64 { self.inner.config.time_height }
    #[setter]
    fn set_time_height(&mut self, v: f64) { self.inner.config.time_height = v; }

    #[getter]
    fn lane_width(&self) -> i32 { self.inner.config.lane_width }
    #[setter]
    fn set_lane_width(&mut self, v: i32) { self.inner.config.lane_width = v; }
}

/// Convenience function: parse a .sus file and generate SVG in one call
#[pyfunction]
#[pyo3(signature = (sus_path, note_host=None, style_sheet=None, rebase_json=None, lyric_content=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None))]
fn sus_to_svg(
    sus_path: &str,
    note_host: Option<String>,
    style_sheet: Option<String>,
    rebase_json: Option<&str>,
    lyric_content: Option<&str>,
    skill: bool,
    music_meta: Option<&Bound<'_, PyDict>>,
    target_segment_seconds: Option<f64>,
    generator: Option<String>,
) -> PyResult<String> {
    let mut score = Score::open(sus_path).map_err(|e| {
        pyo3::exceptions::PyIOError::new_err(format!("Failed to open score: {e}"))
    })?;

    if let Some(json_str) = rebase_json {
        let rebase = Rebase::from_json(json_str).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid rebase JSON: {e}"))
        })?;
        score = rebase.apply(&mut score);
    }

    let lyric = lyric_content.map(Lyric::load);

    let mm = if let Some(meta_dict) = music_meta {
        Some(MusicMeta {
            fever_end_time: meta_dict.get_item("fever_end_time")?.map(|v| v.extract::<f64>()).transpose()?.unwrap_or(0.0),
            fever_score: meta_dict.get_item("fever_score")?.map(|v| v.extract::<f64>()).transpose()?.unwrap_or(0.0),
            skill_score_solo: meta_dict.get_item("skill_score_solo")?.map(|v| v.extract::<Vec<f64>>()).transpose()?.unwrap_or_default(),
            skill_score_multi: meta_dict.get_item("skill_score_multi")?.map(|v| v.extract::<Vec<f64>>()).transpose()?.unwrap_or_default(),
        })
    } else {
        None
    };

    let mut drawing = Drawing::new(note_host, style_sheet, skill, mm, target_segment_seconds, generator);
    Ok(drawing.svg(&mut score, lyric.as_ref()))
}

/// Register all Python types and functions
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyEvent>()?;
    m.add_class::<PyScore>()?;
    m.add_class::<PyLyric>()?;
    m.add_class::<PyRebase>()?;
    m.add_class::<PyDrawing>()?;
    m.add_function(wrap_pyfunction!(sus_to_svg, m)?)?;
    Ok(())
}
