/// Chart metadata matching Python's Meta dataclass
#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub artist: Option<String>,
    pub genre: Option<String>,
    pub designer: Option<String>,
    pub difficulty: Option<String>,
    pub playlevel: Option<String>,
    pub songid: Option<String>,
    pub wave: Option<String>,
    pub waveoffset: Option<String>,
    pub jacket: Option<String>,
    pub background: Option<String>,
    pub movie: Option<String>,
    pub movieoffset: Option<f64>,
    pub basebpm: Option<f64>,
}

impl Meta {
    pub fn new() -> Self {
        Meta::default()
    }

    /// Merge operator matching Python's `__or__`: self takes priority using Python's
    /// falsy semantics (empty strings are treated as missing, matching `self.x or other.x`)
    pub fn merge(&self, other: &Meta) -> Meta {
        Meta {
            title: or_falsy(&self.title, &other.title),
            subtitle: or_falsy(&self.subtitle, &other.subtitle),
            artist: or_falsy(&self.artist, &other.artist),
            genre: or_falsy(&self.genre, &other.genre),
            designer: or_falsy(&self.designer, &other.designer),
            difficulty: or_falsy(&self.difficulty, &other.difficulty),
            playlevel: or_falsy(&self.playlevel, &other.playlevel),
            songid: or_falsy(&self.songid, &other.songid),
            wave: or_falsy(&self.wave, &other.wave),
            waveoffset: or_falsy(&self.waveoffset, &other.waveoffset),
            jacket: or_falsy(&self.jacket, &other.jacket),
            background: or_falsy(&self.background, &other.background),
            movie: or_falsy(&self.movie, &other.movie),
            movieoffset: self.movieoffset.or(other.movieoffset),
            basebpm: self.basebpm.or(other.basebpm),
        }
    }

    /// Set a metadata field by lowercase name (matching Python's setattr pattern)
    pub fn set_field(&mut self, name: &str, value: &str) {
        match name {
            "title" => self.title = Some(value.to_string()),
            "subtitle" => self.subtitle = Some(value.to_string()),
            "artist" => self.artist = Some(value.to_string()),
            "genre" => self.genre = Some(value.to_string()),
            "designer" => self.designer = Some(value.to_string()),
            "difficulty" => self.difficulty = Some(value.to_string()),
            "playlevel" => self.playlevel = Some(value.to_string()),
            "songid" => self.songid = Some(value.to_string()),
            "wave" => self.wave = Some(value.to_string()),
            "waveoffset" => self.waveoffset = Some(value.to_string()),
            "jacket" => self.jacket = Some(value.to_string()),
            "background" => self.background = Some(value.to_string()),
            "movie" => self.movie = Some(value.to_string()),
            "movieoffset" => {
                if let Ok(v) = value.parse::<f64>() {
                    self.movieoffset = Some(v);
                }
            }
            "basebpm" => {
                if let Ok(v) = value.parse::<f64>() {
                    self.basebpm = Some(v);
                }
            }
            _ => {}
        }
    }

    /// Check if a field name is valid
    pub fn has_field(name: &str) -> bool {
        matches!(
            name,
            "title"
                | "subtitle"
                | "artist"
                | "genre"
                | "designer"
                | "difficulty"
                | "playlevel"
                | "songid"
                | "wave"
                | "waveoffset"
                | "jacket"
                | "background"
                | "movie"
                | "movieoffset"
                | "basebpm"
        )
    }
}

/// Python-style falsy merge for Option<String>: None and empty strings fall through
fn or_falsy(a: &Option<String>, b: &Option<String>) -> Option<String> {
    match a {
        Some(s) if !s.is_empty() => Some(s.clone()),
        _ => b.clone(),
    }
}
