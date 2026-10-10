//! System font fallback for the tiny-skia backend (feature `system-fonts`).
//!
//! Skia resolves CSS families it cannot find among the registered fonts
//! through the platform font manager (fontconfig on Linux). This module does
//! the same with fontdb: the system font directories are scanned once per
//! process, on the first lookup that misses the registered fonts, and matched
//! faces are loaded and kept for the life of the process.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use fontdb::{Database, Family, ID, Query, Stretch, Style, Weight};

use super::text::{FontStyle, Slant, Typeface, Unichar, load_typefaces_from_data};

static DATABASE: LazyLock<Database> = LazyLock::new(|| {
    let mut database = Database::new();
    database.load_system_fonts();
    database
});

/// Loaded faces by fontdb id (`None` when the face could not be parsed).
static FACES: LazyLock<Mutex<HashMap<ID, Option<Typeface>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Families tried for `sans-serif` (and as the last resort) when fontdb's
/// configured generic family is not installed.
const SANS_SERIF_FALLBACKS: &[&str] = &[
    "DejaVu Sans",
    "Noto Sans",
    "Liberation Sans",
    "Arial",
    "Helvetica",
    "Noto Sans CJK JP",
    "Noto Sans CJK SC",
];

/// Whether any system font is available.
pub(super) fn available() -> bool {
    !DATABASE.is_empty()
}

/// The system face that best matches `family` and `style` and covers
/// `required_glyphs`, like `SkFontMgr::matchFamilyStyle`.
pub(super) fn match_family(
    family: &str,
    style: FontStyle,
    required_glyphs: &[Unichar],
) -> Option<Typeface> {
    let generic = match family.to_ascii_lowercase().as_str() {
        "sans-serif" | "system-ui" => Some(Family::SansSerif),
        "serif" => Some(Family::Serif),
        "monospace" => Some(Family::Monospace),
        "cursive" => Some(Family::Cursive),
        "fantasy" => Some(Family::Fantasy),
        _ => None,
    };
    let query_family = generic.unwrap_or(Family::Name(family));
    query(&[query_family], style, required_glyphs).or_else(|| {
        matches!(generic, Some(Family::SansSerif))
            .then(|| default_typeface(style, required_glyphs))
            .flatten()
    })
}

/// A reasonable default sans-serif face (`SkFontMgr::legacyMakeTypeface(nullptr, ..)`).
pub(super) fn default_typeface(style: FontStyle, required_glyphs: &[Unichar]) -> Option<Typeface> {
    std::iter::once(Family::SansSerif)
        .chain(SANS_SERIF_FALLBACKS.iter().map(|name| Family::Name(name)))
        .find_map(|family| query(&[family], style, required_glyphs))
}

fn query(
    families: &[Family<'_>],
    style: FontStyle,
    required_glyphs: &[Unichar],
) -> Option<Typeface> {
    let id = DATABASE.query(&Query {
        families,
        weight: Weight(style.weight().clamp(1, 1000) as u16),
        stretch: stretch(style.width()),
        style: match style.slant() {
            Slant::Upright => Style::Normal,
            Slant::Italic => Style::Italic,
            Slant::Oblique => Style::Oblique,
        },
    })?;
    let typeface = load(id)?;
    required_glyphs
        .iter()
        .all(|&glyph| typeface.unichar_to_glyph(glyph) != 0)
        .then_some(typeface)
}

fn load(id: ID) -> Option<Typeface> {
    let mut faces = FACES.lock().expect("system font cache lock poisoned");
    faces
        .entry(id)
        .or_insert_with(|| {
            DATABASE
                .with_face_data(id, |data, index| {
                    load_typefaces_from_data(data.to_vec())
                        .into_iter()
                        .nth(index as usize)
                })
                .flatten()
        })
        .clone()
}

fn stretch(width: i32) -> Stretch {
    match width {
        ..=1 => Stretch::UltraCondensed,
        2 => Stretch::ExtraCondensed,
        3 => Stretch::Condensed,
        4 => Stretch::SemiCondensed,
        5 => Stretch::Normal,
        6 => Stretch::SemiExpanded,
        7 => Stretch::Expanded,
        8 => Stretch::ExtraExpanded,
        _ => Stretch::UltraExpanded,
    }
}

#[cfg(test)]
mod tests {
    use super::{available, default_typeface, match_family};
    use crate::tiny_skia_direct::text::FontStyle;

    #[test]
    fn resolves_generic_and_named_families_when_fonts_are_installed() {
        if !available() {
            eprintln!("skipped: no system fonts");
            return;
        }
        let sans = match_family("sans-serif", FontStyle::normal(), &['A' as i32]);
        assert!(sans.is_some(), "sans-serif did not resolve");
        assert!(default_typeface(FontStyle::bold(), &[]).is_some());
        assert!(match_family("No Such Family 1f2e3d", FontStyle::normal(), &[]).is_none());
    }
}
