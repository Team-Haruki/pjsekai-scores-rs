use clap::{Parser, ValueEnum};
use pjsekai_scores_rs::{Drawing, Lyric, MusicMeta, Rebase, Score};
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(
    name = "pjsekai-scores",
    about = "Project SEKAI score (.sus/custom JSON) to SVG/PNG/JPEG renderer"
)]
struct Args {
    /// The score file (.sus or Project SEKAI custom chart JSON)
    score: String,

    /// Input score format
    #[arg(long, default_value = "auto")]
    score_format: ScoreFormat,

    /// Customized BPM, beats and sections (JSON)
    #[arg(long)]
    rebase: Option<String>,

    /// Lyrics file
    #[arg(long)]
    lyric: Option<String>,

    /// Custom CSS stylesheet
    #[arg(long)]
    css: Option<String>,

    /// Base URL for SVG note assets, or local directory for Skia image note assets
    #[arg(long, default_value = "https://asset3.pjsekai.moe/live/note/custom01")]
    note_host: String,

    /// File extension for note asset files
    #[arg(long, default_value = "png")]
    note_asset_extension: String,

    /// Music title shown in the chart footer
    #[arg(long)]
    title: Option<String>,

    /// Music artist shown in the chart footer
    #[arg(long)]
    artist: Option<String>,

    /// Difficulty shown in the chart footer
    #[arg(long)]
    difficulty: Option<String>,

    /// Play level shown in the chart footer
    #[arg(long)]
    play_level: Option<String>,

    /// Music ID shown in the chart footer
    #[arg(long)]
    music_id: Option<String>,

    /// Jacket image URI/path shown in the chart footer
    #[arg(long)]
    jacket: Option<String>,

    /// Render skill and fever overlay coverage
    #[arg(long)]
    skill: bool,

    /// Music metadata JSON or JSON file path for skill score overlay
    #[arg(long)]
    music_meta: Option<String>,

    /// Approximate seconds per chart column
    #[arg(long)]
    target_segment_seconds: Option<f64>,

    /// JPEG quality for .jpg/.jpeg output (0-100)
    #[arg(long, default_value_t = 90, value_parser = parse_jpeg_quality)]
    jpeg_quality: u8,

    /// Output file path (.svg, .png, .jpg, or .jpeg)
    #[arg(short, long)]
    output: Option<String>,

    /// Generator name shown in the SVG subtitle
    #[arg(long)]
    generator: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let output = resolve_output_path(&args);

    // Parse score
    let mut score = open_score(&args)?;
    apply_cli_meta(&mut score, &args);

    // Apply rebase if provided
    if let Some(rebase_path) = &args.rebase {
        let json_str = fs::read_to_string(rebase_path)?;
        let rebase = Rebase::from_json(&json_str)?;
        score = rebase.apply(&mut score);
    }

    // Load lyrics
    let lyric = if let Some(lyric_path) = &args.lyric {
        let content = fs::read_to_string(lyric_path)?;
        Some(Lyric::load(&content))
    } else {
        None
    };

    // Load custom CSS
    let custom_css = if let Some(css_path) = &args.css {
        Some(fs::read_to_string(css_path)?)
    } else {
        None
    };

    let music_meta = match &args.music_meta {
        Some(value) => Some(parse_music_meta(value)?),
        None => None,
    };

    // Generate output
    let mut drawing = Drawing::new(
        Some(args.note_host),
        custom_css,
        args.skill,
        music_meta,
        args.target_segment_seconds,
        args.generator,
    );
    drawing.set_note_asset_extension(args.note_asset_extension);

    match output_format(&output)? {
        OutputFormat::Svg => {
            let svg = drawing.svg(&mut score, lyric.as_ref());
            fs::write(&output, svg)?;
        }
        OutputFormat::Png => {
            write_skia_image_output(
                &output,
                OutputFormat::Png,
                args.jpeg_quality,
                &mut drawing,
                &mut score,
                lyric.as_ref(),
            )?;
        }
        OutputFormat::Jpeg => {
            write_skia_image_output(
                &output,
                OutputFormat::Jpeg,
                args.jpeg_quality,
                &mut drawing,
                &mut score,
                lyric.as_ref(),
            )?;
        }
    }
    eprintln!("Written to {output}");

    Ok(())
}

fn resolve_output_path(args: &Args) -> String {
    if let Some(out) = &args.output {
        if Path::new(out).is_dir() {
            let stem = Path::new(&args.score)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            format!("{}/{stem}.svg", out)
        } else {
            out.clone()
        }
    } else {
        let stem = Path::new(&args.score)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let dir = Path::new(&args.score).parent().unwrap_or(Path::new("."));
        format!("{}/{stem}.svg", dir.display())
    }
}

#[cfg(feature = "skia-image")]
fn write_skia_image_output(
    output: &str,
    output_format: OutputFormat,
    jpeg_quality: u8,
    drawing: &mut Drawing,
    score: &mut Score,
    lyric: Option<&Lyric>,
) -> Result<(), Box<dyn std::error::Error>> {
    use pjsekai_scores_rs::{SkiaImageFormat, score_to_skia_image};

    let skia_format = match output_format {
        OutputFormat::Png => SkiaImageFormat::Png,
        OutputFormat::Jpeg => SkiaImageFormat::Jpeg {
            quality: jpeg_quality,
        },
        OutputFormat::Svg => unreachable!("SVG output does not use Skia"),
    };
    let bytes = score_to_skia_image(drawing, score, lyric, skia_format)?;
    fs::write(output, bytes)?;
    Ok(())
}

#[cfg(not(feature = "skia-image"))]
fn write_skia_image_output(
    _output: &str,
    _output_format: OutputFormat,
    _jpeg_quality: u8,
    _drawing: &mut Drawing,
    _score: &mut Score,
    _lyric: Option<&Lyric>,
) -> Result<(), Box<dyn std::error::Error>> {
    Err("PNG/JPEG output requires building with `--features skia-image`".into())
}

#[derive(Debug, Clone, Copy)]
enum OutputFormat {
    Svg,
    Png,
    Jpeg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ScoreFormat {
    Auto,
    Sus,
    Json,
}

fn open_score(args: &Args) -> Result<Score, Box<dyn std::error::Error>> {
    match args.score_format {
        ScoreFormat::Auto => Ok(Score::open(&args.score)?),
        ScoreFormat::Sus => Ok(Score::open_sus(&args.score)?),
        ScoreFormat::Json => Ok(Score::open_json(&args.score)?),
    }
}

fn output_format(output: &str) -> Result<OutputFormat, Box<dyn std::error::Error>> {
    match Path::new(output)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Ok(OutputFormat::Png),
        Some("jpg") | Some("jpeg") => Ok(OutputFormat::Jpeg),
        Some("svg") | None => Ok(OutputFormat::Svg),
        Some(ext) => Err(format!(
            "unsupported output extension `{ext}`; use .svg, .png, .jpg, or .jpeg"
        )
        .into()),
    }
}

fn parse_jpeg_quality(value: &str) -> Result<u8, String> {
    let quality = value
        .parse::<u8>()
        .map_err(|_| "JPEG quality must be an integer from 0 to 100".to_string())?;
    if quality <= 100 {
        Ok(quality)
    } else {
        Err("JPEG quality must be an integer from 0 to 100".to_string())
    }
}

fn apply_cli_meta(score: &mut Score, args: &Args) {
    if let Some(value) = &args.title {
        score.meta.title = Some(value.clone());
    }
    if let Some(value) = &args.artist {
        score.meta.artist = Some(value.clone());
    }
    if let Some(value) = &args.difficulty {
        score.meta.difficulty = Some(value.clone());
    }
    if let Some(value) = &args.play_level {
        score.meta.playlevel = Some(value.clone());
    }
    if let Some(value) = &args.music_id {
        score.meta.songid = Some(value.clone());
    }
    if let Some(value) = &args.jacket {
        score.meta.jacket = Some(value.clone());
    }
}

fn parse_music_meta(value: &str) -> Result<MusicMeta, Box<dyn std::error::Error>> {
    let json = match fs::read_to_string(value) {
        Ok(content) => content,
        Err(_) => value.to_string(),
    };
    let parsed: Value = serde_json::from_str(&json)?;
    let get_f64 = |key: &str| parsed.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let get_f64_vec = |key: &str| {
        parsed
            .get(key)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default()
    };
    Ok(MusicMeta {
        fever_end_time: get_f64("fever_end_time"),
        fever_score: get_f64("fever_score"),
        skill_score_solo: get_f64_vec("skill_score_solo"),
        skill_score_multi: get_f64_vec("skill_score_multi"),
    })
}
