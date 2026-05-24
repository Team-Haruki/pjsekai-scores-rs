use pyo3::PyRef;
use pyo3::PyRefMut;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::types::PyDict;
use pyo3::types::PyModule;

use crate::drawing::{Drawing, MusicMeta};
use crate::fraction::Fraction;
use crate::lyric::Lyric;
use crate::meta::Meta;
use crate::notes::event::Event;
use crate::rebase::Rebase;
use crate::score::Score;

fn extract_fraction_like(value: &Bound<'_, pyo3::types::PyAny>) -> PyResult<Fraction> {
    if let Ok(py_fraction) = value.extract::<PyRef<'_, PyFraction>>() {
        return Ok(py_fraction.inner);
    }
    if let Ok(n) = value.extract::<i64>() {
        return Ok(Fraction::from_integer(n));
    }
    if let Ok(f) = value.extract::<f64>() {
        return Ok(Fraction::from_f64(f));
    }
    if let Ok(s) = value.extract::<String>()
        && let Some(f) = Fraction::parse(&s)
    {
        return Ok(f);
    }

    Err(pyo3::exceptions::PyTypeError::new_err(
        "expected a Fraction-compatible value",
    ))
}

fn read_text_or_file(value: &Bound<'_, pyo3::types::PyAny>) -> PyResult<String> {
    if let Ok(s) = value.extract::<String>() {
        return Ok(s);
    }
    if value.hasattr("read")? {
        return value.call_method0("read")?.extract::<String>();
    }
    if value.hasattr("readlines")? {
        let lines: Vec<String> = value.call_method0("readlines")?.extract()?;
        return Ok(lines.concat());
    }

    Err(pyo3::exceptions::PyTypeError::new_err(
        "expected a string or a file-like object",
    ))
}

fn parse_music_meta(meta_dict: Option<&Bound<'_, PyDict>>) -> PyResult<Option<MusicMeta>> {
    let Some(meta_dict) = meta_dict else {
        return Ok(None);
    };

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

    Ok(Some(MusicMeta {
        fever_end_time,
        fever_score,
        skill_score_solo,
        skill_score_multi,
    }))
}

fn open_score_for_render(sus_path: &str, rebase_json: Option<&str>) -> PyResult<Score> {
    let mut score = Score::open(sus_path)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Failed to open score: {e}")))?;

    if let Some(json_str) = rebase_json {
        let rebase = Rebase::from_json(json_str).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid rebase JSON: {e}"))
        })?;
        score = rebase.apply(&mut score);
    }

    Ok(score)
}

#[allow(clippy::too_many_arguments)]
fn drawing_for_render(
    note_host: Option<String>,
    style_sheet: Option<String>,
    skill: bool,
    music_meta: Option<&Bound<'_, PyDict>>,
    target_segment_seconds: Option<f64>,
    generator: Option<String>,
    note_asset_extension: Option<String>,
    font_paths: Option<Vec<String>>,
    font_dirs: Option<Vec<String>>,
) -> PyResult<Drawing> {
    let mm = parse_music_meta(music_meta)?;
    let mut drawing = Drawing::new(
        note_host,
        style_sheet,
        skill,
        mm,
        target_segment_seconds,
        generator,
    );
    if let Some(extension) = note_asset_extension {
        drawing.set_note_asset_extension(extension);
    }
    if let Some(paths) = font_paths {
        drawing.set_font_paths(paths);
    }
    if let Some(dirs) = font_dirs {
        drawing.set_font_dirs(dirs);
    }
    Ok(drawing)
}

#[cfg(feature = "skia-image")]
fn render_png_bytes(
    drawing: &mut Drawing,
    score: &mut Score,
    lyric: Option<&Lyric>,
) -> PyResult<Vec<u8>> {
    crate::score_to_skia_png(drawing, score, lyric).map_err(|e| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to render PNG: {e}"))
    })
}

#[cfg(feature = "skia-image")]
fn render_jpeg_bytes(
    drawing: &mut Drawing,
    score: &mut Score,
    lyric: Option<&Lyric>,
    quality: u8,
) -> PyResult<Vec<u8>> {
    let quality = checked_jpeg_quality(quality)?;
    crate::score_to_skia_jpeg(drawing, score, lyric, quality).map_err(|e| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to render JPEG: {e}"))
    })
}

#[cfg(not(feature = "skia-image"))]
fn render_png_bytes(
    _drawing: &mut Drawing,
    _score: &mut Score,
    _lyric: Option<&Lyric>,
) -> PyResult<Vec<u8>> {
    Err(pyo3::exceptions::PyRuntimeError::new_err(
        "PNG/JPEG output requires the `skia-image` feature; install `pjsekai-scores-rs-skia-image` or build with `--features python,skia-image`",
    ))
}

#[cfg(not(feature = "skia-image"))]
fn render_jpeg_bytes(
    _drawing: &mut Drawing,
    _score: &mut Score,
    _lyric: Option<&Lyric>,
    _quality: u8,
) -> PyResult<Vec<u8>> {
    Err(pyo3::exceptions::PyRuntimeError::new_err(
        "PNG/JPEG output requires the `skia-image` feature; install `pjsekai-scores-rs-skia-image` or build with `--features python,skia-image`",
    ))
}

#[cfg(feature = "skia-image")]
fn checked_jpeg_quality(quality: u8) -> PyResult<u8> {
    if quality <= 100 {
        Ok(quality)
    } else {
        Err(pyo3::exceptions::PyValueError::new_err(
            "jpeg_quality must be an integer from 0 to 100",
        ))
    }
}

#[pyclass(name = "Fraction", skip_from_py_object)]
#[derive(Clone)]
struct PyFraction {
    inner: Fraction,
}

#[pymethods]
impl PyFraction {
    #[new]
    #[pyo3(signature = (numerator, denominator=1))]
    fn new(numerator: i64, denominator: i64) -> Self {
        Self {
            inner: Fraction::new(numerator, denominator),
        }
    }

    #[getter]
    fn numerator(&self) -> i64 {
        *self.inner.numer()
    }

    #[getter]
    fn denominator(&self) -> i64 {
        *self.inner.denom()
    }

    fn limit_denominator(&self, max_denominator: Option<i64>) -> PyFraction {
        PyFraction {
            inner: self
                .inner
                .limit_denominator(max_denominator.unwrap_or(1_000_000)),
        }
    }

    fn __float__(&self) -> f64 {
        self.inner.to_f64()
    }

    fn __str__(&self) -> String {
        self.inner.to_string()
    }

    fn __repr__(&self) -> String {
        self.inner.to_string()
    }
}

#[pyclass(name = "Meta", skip_from_py_object)]
#[derive(Clone)]
struct PyMeta {
    inner: Meta,
}

#[pymethods]
impl PyMeta {
    #[getter]
    fn title(&self) -> Option<String> {
        self.inner.title.clone()
    }

    #[getter]
    fn subtitle(&self) -> Option<String> {
        self.inner.subtitle.clone()
    }

    #[getter]
    fn artist(&self) -> Option<String> {
        self.inner.artist.clone()
    }

    #[getter]
    fn genre(&self) -> Option<String> {
        self.inner.genre.clone()
    }

    #[getter]
    fn designer(&self) -> Option<String> {
        self.inner.designer.clone()
    }

    #[getter]
    fn difficulty(&self) -> Option<String> {
        self.inner.difficulty.clone()
    }

    #[getter]
    fn playlevel(&self) -> Option<String> {
        self.inner.playlevel.clone()
    }

    #[getter]
    fn songid(&self) -> Option<String> {
        self.inner.songid.clone()
    }

    #[getter]
    fn wave(&self) -> Option<String> {
        self.inner.wave.clone()
    }

    #[getter]
    fn waveoffset(&self) -> Option<String> {
        self.inner.waveoffset.clone()
    }

    #[getter]
    fn jacket(&self) -> Option<String> {
        self.inner.jacket.clone()
    }

    #[getter]
    fn background(&self) -> Option<String> {
        self.inner.background.clone()
    }

    #[getter]
    fn movie(&self) -> Option<String> {
        self.inner.movie.clone()
    }

    #[getter]
    fn movieoffset(&self) -> Option<f64> {
        self.inner.movieoffset
    }

    #[getter]
    fn basebpm(&self) -> Option<f64> {
        self.inner.basebpm
    }
}

/// Lightweight event view exposed to Python
#[pyclass(name = "Event", skip_from_py_object)]
#[derive(Clone)]
struct PyEvent {
    inner: Event,
}

#[pymethods]
impl PyEvent {
    #[getter]
    fn bar(&self) -> PyFraction {
        PyFraction {
            inner: self.inner.bar,
        }
    }

    #[getter]
    fn bpm(&self) -> Option<PyFraction> {
        self.inner.bpm.map(|inner| PyFraction { inner })
    }

    #[getter]
    fn bar_length(&self) -> Option<PyFraction> {
        self.inner.bar_length.map(|inner| PyFraction { inner })
    }

    #[getter]
    fn sentence_length(&self) -> Option<i32> {
        self.inner.sentence_length
    }

    #[getter]
    fn speed(&self) -> Option<f64> {
        self.inner.speed
    }

    #[getter]
    fn section(&self) -> Option<String> {
        self.inner.section.clone()
    }

    #[getter]
    fn text(&self) -> Option<String> {
        self.inner.text.clone()
    }
}

/// Python wrapper for Score
#[pyclass(name = "Score")]
struct PyScore {
    inner: Score,
}

#[pymethods]
impl PyScore {
    /// Open and parse a score file (.sus or Project SEKAI custom chart JSON)
    #[staticmethod]
    fn open(path: &str) -> PyResult<PyScore> {
        let score = Score::open(path).map_err(|e| {
            pyo3::exceptions::PyIOError::new_err(format!("Failed to open score: {e}"))
        })?;
        Ok(PyScore { inner: score })
    }

    /// Open and parse a .sus file
    #[staticmethod]
    fn open_sus(path: &str) -> PyResult<PyScore> {
        let score = Score::open_sus(path).map_err(|e| {
            pyo3::exceptions::PyIOError::new_err(format!("Failed to open SUS score: {e}"))
        })?;
        Ok(PyScore { inner: score })
    }

    /// Open and parse Project SEKAI custom chart JSON
    #[staticmethod]
    fn open_json(path: &str) -> PyResult<PyScore> {
        let score = Score::open_json(path).map_err(|e| {
            pyo3::exceptions::PyIOError::new_err(format!("Failed to open JSON score: {e}"))
        })?;
        Ok(PyScore { inner: score })
    }

    /// Parse from .sus string content
    #[staticmethod]
    fn from_str(content: &str) -> PyScore {
        PyScore {
            inner: content.parse().unwrap(),
        }
    }

    /// Parse from Project SEKAI custom chart JSON string, dict, or file-like object
    #[staticmethod]
    fn from_json(py: pyo3::Python<'_>, value: &Bound<'_, pyo3::types::PyAny>) -> PyResult<PyScore> {
        let content = if value.is_instance_of::<PyDict>() {
            py.import("json")?
                .call_method1("dumps", (value,))?
                .extract::<String>()?
        } else {
            read_text_or_file(value)?
        };
        let score = Score::parse_json(&content).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid score JSON: {e}"))
        })?;
        Ok(PyScore { inner: score })
    }

    /// Load from a Python dict
    #[staticmethod]
    fn from_dict(py: pyo3::Python<'_>, dict: &Bound<'_, pyo3::types::PyAny>) -> PyResult<PyScore> {
        Self::from_json(py, dict)
    }

    /// Load from .sus string content, JSON string content, dict, or file-like object
    #[staticmethod]
    fn load(py: pyo3::Python<'_>, value: &Bound<'_, pyo3::types::PyAny>) -> PyResult<PyScore> {
        if value.is_instance_of::<PyDict>() {
            return Self::from_json(py, value);
        }

        let content = read_text_or_file(value)?;
        let score = Score::parse_auto(&content).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("Invalid score JSON: {e}"))
        })?;
        Ok(PyScore { inner: score })
    }

    #[getter]
    fn meta(&self) -> PyMeta {
        PyMeta {
            inner: self.inner.meta.clone(),
        }
    }

    /// Set metadata fields (keyword args, all optional)
    #[pyo3(signature = (title=None, artist=None, difficulty=None, playlevel=None, jacket=None, songid=None, subtitle=None))]
    #[allow(clippy::too_many_arguments)]
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
        if let Some(v) = title {
            self.inner.meta.title = Some(v);
        }
        if let Some(v) = artist {
            self.inner.meta.artist = Some(v);
        }
        if let Some(v) = difficulty {
            self.inner.meta.difficulty = Some(v);
        }
        if let Some(v) = playlevel {
            self.inner.meta.playlevel = Some(v);
        }
        if let Some(v) = jacket {
            self.inner.meta.jacket = Some(v);
        }
        if let Some(v) = songid {
            self.inner.meta.songid = Some(v);
        }
        if let Some(v) = subtitle {
            self.inner.meta.subtitle = Some(v);
        }
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
        self.inner
            .events
            .iter()
            .cloned()
            .map(|inner| PyEvent { inner })
            .collect()
    }

    fn get_time(&mut self, bar: &Bound<'_, pyo3::types::PyAny>) -> PyResult<PyFraction> {
        let bar = extract_fraction_like(bar)?;
        Ok(PyFraction {
            inner: self.inner.get_time(bar),
        })
    }

    fn get_event(&mut self, bar: &Bound<'_, pyo3::types::PyAny>) -> PyResult<PyEvent> {
        let bar = extract_fraction_like(bar)?;
        Ok(PyEvent {
            inner: self.inner.get_event(bar),
        })
    }

    fn get_time_delta(
        &mut self,
        bar_from: &Bound<'_, pyo3::types::PyAny>,
        bar_to: &Bound<'_, pyo3::types::PyAny>,
    ) -> PyResult<PyFraction> {
        let bar_from = extract_fraction_like(bar_from)?;
        let bar_to = extract_fraction_like(bar_to)?;
        Ok(PyFraction {
            inner: self.inner.get_time_delta(bar_from, bar_to),
        })
    }

    fn get_bar_by_time(&mut self, time: f64) -> PyFraction {
        PyFraction {
            inner: self.inner.get_bar_by_time(time),
        }
    }
}

/// Python wrapper for Lyric
#[pyclass(name = "Lyric")]
struct PyLyric {
    inner: Lyric,
}

#[pymethods]
impl PyLyric {
    /// Load lyrics from a string or a file-like object
    #[staticmethod]
    fn load(content: &Bound<'_, pyo3::types::PyAny>) -> PyResult<PyLyric> {
        let content = read_text_or_file(content)?;
        Ok(PyLyric {
            inner: Lyric::load(&content),
        })
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
    #[staticmethod]
    fn load(py: pyo3::Python<'_>, value: &Bound<'_, pyo3::types::PyAny>) -> PyResult<PyRebase> {
        if value.is_instance_of::<PyDict>() {
            return Self::from_dict(py, value);
        }
        let content = read_text_or_file(value)?;
        Self::from_json(&content)
    }

    /// Load from JSON string
    #[staticmethod]
    fn from_json(json_str: &str) -> PyResult<PyRebase> {
        let rebase = Rebase::from_json(json_str)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("Invalid JSON: {e}")))?;
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

    #[staticmethod]
    fn load_from_dict(
        py: pyo3::Python<'_>,
        dict: &Bound<'_, pyo3::types::PyAny>,
    ) -> PyResult<PyRebase> {
        Self::from_dict(py, dict)
    }

    /// Apply rebase to a score
    fn apply(&self, score: &mut PyScore) -> PyScore {
        let new_score = self.inner.apply(&mut score.inner);
        PyScore { inner: new_score }
    }

    fn rebase(&self, score: &mut PyScore) -> PyScore {
        self.apply(score)
    }

    fn __call__(&self, score: &mut PyScore) -> PyScore {
        self.apply(score)
    }
}

/// Python wrapper for Drawing
#[pyclass(name = "Drawing")]
struct PyDrawing {
    inner: Drawing,
    stored_score: Option<Score>,
    stored_lyric: Option<Lyric>,
}

#[pymethods]
impl PyDrawing {
    #[new]
    #[pyo3(signature = (score=None, lyric=None, style_sheet=None, note_host=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None, note_asset_extension=None, font_paths=None, font_dirs=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        score: Option<PyRef<'_, PyScore>>,
        lyric: Option<PyRef<'_, PyLyric>>,
        style_sheet: Option<String>,
        note_host: Option<String>,
        skill: bool,
        music_meta: Option<&Bound<'_, PyDict>>,
        target_segment_seconds: Option<f64>,
        generator: Option<String>,
        note_asset_extension: Option<String>,
        font_paths: Option<Vec<String>>,
        font_dirs: Option<Vec<String>>,
    ) -> PyResult<PyDrawing> {
        let drawing = drawing_for_render(
            note_host,
            style_sheet,
            skill,
            music_meta,
            target_segment_seconds,
            generator,
            note_asset_extension,
            font_paths,
            font_dirs,
        )?;

        Ok(PyDrawing {
            inner: drawing,
            stored_score: score.map(|score| score.inner.clone()),
            stored_lyric: lyric.map(|lyric| lyric.inner.clone()),
        })
    }

    /// Generate SVG string from a score
    #[pyo3(signature = (score=None, lyric=None))]
    fn svg(
        &mut self,
        score: Option<PyRefMut<'_, PyScore>>,
        lyric: Option<PyRef<'_, PyLyric>>,
    ) -> PyResult<String> {
        let lyric_override = lyric.map(|lyric| lyric.inner.clone());

        if let Some(mut score) = score {
            let lyric_ref = lyric_override.as_ref().or(self.stored_lyric.as_ref());
            return Ok(self.inner.svg(&mut score.inner, lyric_ref));
        }

        let mut stored_score = self.stored_score.clone().ok_or_else(|| {
            pyo3::exceptions::PyTypeError::new_err(
                "score is required when Drawing was created without one",
            )
        })?;
        let lyric_ref = lyric_override.as_ref().or(self.stored_lyric.as_ref());
        Ok(self.inner.svg(&mut stored_score, lyric_ref))
    }

    /// Generate PNG bytes from a score via direct Skia rendering
    #[pyo3(signature = (score=None, lyric=None))]
    fn png<'py>(
        &mut self,
        py: Python<'py>,
        score: Option<PyRefMut<'_, PyScore>>,
        lyric: Option<PyRef<'_, PyLyric>>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let lyric_override = lyric.map(|lyric| lyric.inner.clone());

        let bytes = if let Some(mut score) = score {
            let lyric_ref = lyric_override.as_ref().or(self.stored_lyric.as_ref());
            render_png_bytes(&mut self.inner, &mut score.inner, lyric_ref)?
        } else {
            let mut stored_score = self.stored_score.clone().ok_or_else(|| {
                pyo3::exceptions::PyTypeError::new_err(
                    "score is required when Drawing was created without one",
                )
            })?;
            let lyric_ref = lyric_override.as_ref().or(self.stored_lyric.as_ref());
            render_png_bytes(&mut self.inner, &mut stored_score, lyric_ref)?
        };

        Ok(PyBytes::new(py, &bytes))
    }

    /// Generate JPEG bytes from a score via direct Skia rendering
    #[pyo3(signature = (score=None, lyric=None, jpeg_quality=90))]
    fn jpg<'py>(
        &mut self,
        py: Python<'py>,
        score: Option<PyRefMut<'_, PyScore>>,
        lyric: Option<PyRef<'_, PyLyric>>,
        jpeg_quality: u8,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let lyric_override = lyric.map(|lyric| lyric.inner.clone());

        let bytes = if let Some(mut score) = score {
            let lyric_ref = lyric_override.as_ref().or(self.stored_lyric.as_ref());
            render_jpeg_bytes(&mut self.inner, &mut score.inner, lyric_ref, jpeg_quality)?
        } else {
            let mut stored_score = self.stored_score.clone().ok_or_else(|| {
                pyo3::exceptions::PyTypeError::new_err(
                    "score is required when Drawing was created without one",
                )
            })?;
            let lyric_ref = lyric_override.as_ref().or(self.stored_lyric.as_ref());
            render_jpeg_bytes(&mut self.inner, &mut stored_score, lyric_ref, jpeg_quality)?
        };

        Ok(PyBytes::new(py, &bytes))
    }

    /// Generate JPEG bytes from a score via direct Skia rendering
    #[pyo3(signature = (score=None, lyric=None, jpeg_quality=90))]
    fn jpeg<'py>(
        &mut self,
        py: Python<'py>,
        score: Option<PyRefMut<'_, PyScore>>,
        lyric: Option<PyRef<'_, PyLyric>>,
        jpeg_quality: u8,
    ) -> PyResult<Bound<'py, PyBytes>> {
        self.jpg(py, score, lyric, jpeg_quality)
    }

    #[getter]
    fn note_size(&self) -> i32 {
        self.inner.config.note_size
    }
    #[setter]
    fn set_note_size(&mut self, v: i32) {
        self.inner.config.note_size = v;
    }

    #[getter]
    fn time_height(&self) -> f64 {
        self.inner.config.time_height
    }
    #[setter]
    fn set_time_height(&mut self, v: f64) {
        self.inner.config.time_height = v;
    }

    fn set_font_paths(&mut self, paths: Vec<String>) {
        self.inner.set_font_paths(paths);
    }

    fn add_font_path(&mut self, path: String) {
        self.inner.add_font_path(path);
    }

    fn set_font_dirs(&mut self, dirs: Vec<String>) {
        self.inner.set_font_dirs(dirs);
    }

    fn add_font_dir(&mut self, dir: String) {
        self.inner.add_font_dir(dir);
    }

    #[getter]
    fn lane_width(&self) -> i32 {
        self.inner.config.lane_width
    }
    #[setter]
    fn set_lane_width(&mut self, v: i32) {
        self.inner.config.lane_width = v;
    }
}

/// Convenience function: parse a score file and generate SVG in one call
#[pyfunction]
#[pyo3(signature = (sus_path, note_host=None, style_sheet=None, rebase_json=None, lyric_content=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None, note_asset_extension=None, font_paths=None, font_dirs=None))]
#[allow(clippy::too_many_arguments)]
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
    note_asset_extension: Option<String>,
    font_paths: Option<Vec<String>>,
    font_dirs: Option<Vec<String>>,
) -> PyResult<String> {
    let mut score = open_score_for_render(sus_path, rebase_json)?;
    let lyric = lyric_content.map(Lyric::load);
    let mut drawing = drawing_for_render(
        note_host,
        style_sheet,
        skill,
        music_meta,
        target_segment_seconds,
        generator,
        note_asset_extension,
        font_paths,
        font_dirs,
    )?;
    Ok(drawing.svg(&mut score, lyric.as_ref()))
}

/// Convenience function: parse a score file and generate PNG bytes in one call
#[pyfunction]
#[pyo3(signature = (sus_path, note_host=None, style_sheet=None, rebase_json=None, lyric_content=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None, note_asset_extension=None, font_paths=None, font_dirs=None))]
#[allow(clippy::too_many_arguments)]
fn sus_to_png<'py>(
    py: Python<'py>,
    sus_path: &str,
    note_host: Option<String>,
    style_sheet: Option<String>,
    rebase_json: Option<&str>,
    lyric_content: Option<&str>,
    skill: bool,
    music_meta: Option<&Bound<'_, PyDict>>,
    target_segment_seconds: Option<f64>,
    generator: Option<String>,
    note_asset_extension: Option<String>,
    font_paths: Option<Vec<String>>,
    font_dirs: Option<Vec<String>>,
) -> PyResult<Bound<'py, PyBytes>> {
    let mut score = open_score_for_render(sus_path, rebase_json)?;
    let lyric = lyric_content.map(Lyric::load);
    let mut drawing = drawing_for_render(
        note_host,
        style_sheet,
        skill,
        music_meta,
        target_segment_seconds,
        generator,
        note_asset_extension,
        font_paths,
        font_dirs,
    )?;
    let bytes = render_png_bytes(&mut drawing, &mut score, lyric.as_ref())?;
    Ok(PyBytes::new(py, &bytes))
}

/// Convenience function: parse a score file and generate JPEG bytes in one call
#[pyfunction]
#[pyo3(signature = (sus_path, note_host=None, style_sheet=None, rebase_json=None, lyric_content=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None, note_asset_extension=None, font_paths=None, font_dirs=None, jpeg_quality=90))]
#[allow(clippy::too_many_arguments)]
fn sus_to_jpg<'py>(
    py: Python<'py>,
    sus_path: &str,
    note_host: Option<String>,
    style_sheet: Option<String>,
    rebase_json: Option<&str>,
    lyric_content: Option<&str>,
    skill: bool,
    music_meta: Option<&Bound<'_, PyDict>>,
    target_segment_seconds: Option<f64>,
    generator: Option<String>,
    note_asset_extension: Option<String>,
    font_paths: Option<Vec<String>>,
    font_dirs: Option<Vec<String>>,
    jpeg_quality: u8,
) -> PyResult<Bound<'py, PyBytes>> {
    let mut score = open_score_for_render(sus_path, rebase_json)?;
    let lyric = lyric_content.map(Lyric::load);
    let mut drawing = drawing_for_render(
        note_host,
        style_sheet,
        skill,
        music_meta,
        target_segment_seconds,
        generator,
        note_asset_extension,
        font_paths,
        font_dirs,
    )?;
    let bytes = render_jpeg_bytes(&mut drawing, &mut score, lyric.as_ref(), jpeg_quality)?;
    Ok(PyBytes::new(py, &bytes))
}

/// Convenience function: parse a score file and generate JPEG bytes in one call
#[pyfunction]
#[pyo3(signature = (sus_path, note_host=None, style_sheet=None, rebase_json=None, lyric_content=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None, note_asset_extension=None, font_paths=None, font_dirs=None, jpeg_quality=90))]
#[allow(clippy::too_many_arguments)]
fn sus_to_jpeg<'py>(
    py: Python<'py>,
    sus_path: &str,
    note_host: Option<String>,
    style_sheet: Option<String>,
    rebase_json: Option<&str>,
    lyric_content: Option<&str>,
    skill: bool,
    music_meta: Option<&Bound<'_, PyDict>>,
    target_segment_seconds: Option<f64>,
    generator: Option<String>,
    note_asset_extension: Option<String>,
    font_paths: Option<Vec<String>>,
    font_dirs: Option<Vec<String>>,
    jpeg_quality: u8,
) -> PyResult<Bound<'py, PyBytes>> {
    sus_to_jpg(
        py,
        sus_path,
        note_host,
        style_sheet,
        rebase_json,
        lyric_content,
        skill,
        music_meta,
        target_segment_seconds,
        generator,
        note_asset_extension,
        font_paths,
        font_dirs,
        jpeg_quality,
    )
}

/// Convenience function: parse a score file and generate SVG in one call
#[pyfunction]
#[pyo3(signature = (score_path, note_host=None, style_sheet=None, rebase_json=None, lyric_content=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None, note_asset_extension=None, font_paths=None, font_dirs=None))]
#[allow(clippy::too_many_arguments)]
fn score_to_svg(
    score_path: &str,
    note_host: Option<String>,
    style_sheet: Option<String>,
    rebase_json: Option<&str>,
    lyric_content: Option<&str>,
    skill: bool,
    music_meta: Option<&Bound<'_, PyDict>>,
    target_segment_seconds: Option<f64>,
    generator: Option<String>,
    note_asset_extension: Option<String>,
    font_paths: Option<Vec<String>>,
    font_dirs: Option<Vec<String>>,
) -> PyResult<String> {
    sus_to_svg(
        score_path,
        note_host,
        style_sheet,
        rebase_json,
        lyric_content,
        skill,
        music_meta,
        target_segment_seconds,
        generator,
        note_asset_extension,
        font_paths,
        font_dirs,
    )
}

/// Convenience function: parse a score file and generate PNG bytes in one call
#[pyfunction]
#[pyo3(signature = (score_path, note_host=None, style_sheet=None, rebase_json=None, lyric_content=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None, note_asset_extension=None, font_paths=None, font_dirs=None))]
#[allow(clippy::too_many_arguments)]
fn score_to_png<'py>(
    py: Python<'py>,
    score_path: &str,
    note_host: Option<String>,
    style_sheet: Option<String>,
    rebase_json: Option<&str>,
    lyric_content: Option<&str>,
    skill: bool,
    music_meta: Option<&Bound<'_, PyDict>>,
    target_segment_seconds: Option<f64>,
    generator: Option<String>,
    note_asset_extension: Option<String>,
    font_paths: Option<Vec<String>>,
    font_dirs: Option<Vec<String>>,
) -> PyResult<Bound<'py, PyBytes>> {
    sus_to_png(
        py,
        score_path,
        note_host,
        style_sheet,
        rebase_json,
        lyric_content,
        skill,
        music_meta,
        target_segment_seconds,
        generator,
        note_asset_extension,
        font_paths,
        font_dirs,
    )
}

/// Convenience function: parse a score file and generate JPEG bytes in one call
#[pyfunction]
#[pyo3(signature = (score_path, note_host=None, style_sheet=None, rebase_json=None, lyric_content=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None, note_asset_extension=None, font_paths=None, font_dirs=None, jpeg_quality=90))]
#[allow(clippy::too_many_arguments)]
fn score_to_jpg<'py>(
    py: Python<'py>,
    score_path: &str,
    note_host: Option<String>,
    style_sheet: Option<String>,
    rebase_json: Option<&str>,
    lyric_content: Option<&str>,
    skill: bool,
    music_meta: Option<&Bound<'_, PyDict>>,
    target_segment_seconds: Option<f64>,
    generator: Option<String>,
    note_asset_extension: Option<String>,
    font_paths: Option<Vec<String>>,
    font_dirs: Option<Vec<String>>,
    jpeg_quality: u8,
) -> PyResult<Bound<'py, PyBytes>> {
    sus_to_jpg(
        py,
        score_path,
        note_host,
        style_sheet,
        rebase_json,
        lyric_content,
        skill,
        music_meta,
        target_segment_seconds,
        generator,
        note_asset_extension,
        font_paths,
        font_dirs,
        jpeg_quality,
    )
}

/// Convenience function: parse a score file and generate JPEG bytes in one call
#[pyfunction]
#[pyo3(signature = (score_path, note_host=None, style_sheet=None, rebase_json=None, lyric_content=None, skill=false, music_meta=None, target_segment_seconds=None, generator=None, note_asset_extension=None, font_paths=None, font_dirs=None, jpeg_quality=90))]
#[allow(clippy::too_many_arguments)]
fn score_to_jpeg<'py>(
    py: Python<'py>,
    score_path: &str,
    note_host: Option<String>,
    style_sheet: Option<String>,
    rebase_json: Option<&str>,
    lyric_content: Option<&str>,
    skill: bool,
    music_meta: Option<&Bound<'_, PyDict>>,
    target_segment_seconds: Option<f64>,
    generator: Option<String>,
    note_asset_extension: Option<String>,
    font_paths: Option<Vec<String>>,
    font_dirs: Option<Vec<String>>,
    jpeg_quality: u8,
) -> PyResult<Bound<'py, PyBytes>> {
    score_to_jpg(
        py,
        score_path,
        note_host,
        style_sheet,
        rebase_json,
        lyric_content,
        skill,
        music_meta,
        target_segment_seconds,
        generator,
        note_asset_extension,
        font_paths,
        font_dirs,
        jpeg_quality,
    )
}

/// Register all Python types and functions
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyFraction>()?;
    m.add_class::<PyMeta>()?;
    m.add_class::<PyEvent>()?;
    m.add_class::<PyScore>()?;
    m.add_class::<PyLyric>()?;
    m.add_class::<PyRebase>()?;
    m.add_class::<PyDrawing>()?;
    m.add_function(wrap_pyfunction!(sus_to_svg, m)?)?;
    m.add_function(wrap_pyfunction!(sus_to_png, m)?)?;
    m.add_function(wrap_pyfunction!(sus_to_jpg, m)?)?;
    m.add_function(wrap_pyfunction!(sus_to_jpeg, m)?)?;
    m.add_function(wrap_pyfunction!(score_to_svg, m)?)?;
    m.add_function(wrap_pyfunction!(score_to_png, m)?)?;
    m.add_function(wrap_pyfunction!(score_to_jpg, m)?)?;
    m.add_function(wrap_pyfunction!(score_to_jpeg, m)?)?;
    Ok(())
}
