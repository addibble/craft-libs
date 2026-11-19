//! Checks a downloaded or installed file against its index entry.

use sha2::{Digest, Sha256};
use skrifa::raw::tables::os2::SelectionFlags;
use skrifa::raw::{FileRef, TableProvider};
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};

use crate::index::{Family, FontFile};

/// Tables of which a drawable face has at least one.
const OUTLINE_TABLES: [&[u8; 4]; 4] = [b"glyf", b"CFF ", b"CFF2", b"VARC"];

/// Why a file doesn't match its index entry.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// The size differs.
    #[error("the file is {actual} bytes, not {expected}")]
    Size {
        /// The index's size.
        expected: u64,
        /// The file's.
        actual: u64,
    },
    /// The sha256 differs.
    #[error("the file's sha256 doesn't match the index")]
    Sha256,
    /// It doesn't parse as a font with outlines.
    #[error("the file is not a font, or a damaged one")]
    NotAFont,
    /// No face of the file is the family the entry belongs to.
    #[error("the file is the family {found}, not {expected}")]
    Family {
        /// The entry's family.
        expected: String,
        /// The file's, or `(none)`.
        found: String,
    },
    /// No face of the file has the style the entry claims.
    #[error("the file is {found}, not {expected}")]
    Style {
        /// What the entry claims: `weight 700, upright, width 100%`.
        expected: String,
        /// What the file is.
        found: String,
    },
    /// The file has no variation axis the entry claims, or not its whole range.
    #[error("the file's `{tag}` axis is {found}, not {expected}")]
    Axis {
        /// The axis tag.
        tag: String,
        /// The entry's range.
        expected: String,
        /// The file's range, or `none`.
        found: String,
    },
    /// The entry gives PostScript names and no face of the file has one of them.
    #[error("the file's PostScript name is {found}, not one of {}", expected.join(", "))]
    PostScript {
        /// The entry's PostScript names.
        expected: Vec<String>,
        /// The file's, or `(none)`.
        found: String,
    },
}

/// The sha256 of `bytes`, in lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks `bytes` against `file`: its size, then its sha256, then that every face in it parses
/// and has outlines.
pub fn verify_font(file: &FontFile, bytes: &[u8]) -> Result<(), VerifyError> {
    let actual = bytes.len() as u64;
    if actual != file.size {
        return Err(VerifyError::Size { expected: file.size, actual });
    }
    if sha256_hex(bytes) != file.sha256 {
        return Err(VerifyError::Sha256);
    }
    if !is_font(bytes) {
        return Err(VerifyError::NotAFont);
    }
    Ok(())
}

/// Whether `bytes` is a font file or collection whose every face has a `name` table and outlines.
pub fn is_font(bytes: &[u8]) -> bool {
    let Ok(file) = FileRef::new(bytes) else { return false };
    let mut faces = 0usize;
    for font in file.fonts() {
        let Ok(font) = font else { return false };
        let named = font.table_data(skrifa::Tag::new(b"name")).is_some();
        let drawable = OUTLINE_TABLES.iter().any(|t| font.table_data(skrifa::Tag::new(t)).is_some());
        if !named || !drawable {
            return false;
        }
        faces += 1;
    }
    faces > 0
}

/// [`verify_font`], then that `file` really is what `family`'s index entry says: some face of it
/// (any face of a collection) is
///
/// 1. the entry's family: its typographic family name (name ID 16), or its legacy family name
///    (ID 1) when it has none, equal to `family.family` ignoring case, as the index compares
///    family names;
/// 2. the entry's style: the `OS/2` weight class, width class and italic or oblique flag equal
///    the entry's `weight`, `width` and `style` (a file whose entry has an `ital` axis is not
///    held to the flag), and a file is variable exactly when the entry lists axes, each with at
///    least the range the entry gives;
/// 3. the entry's PostScript name (ID 6), when the entry lists any: one of them, exactly.
///
/// The error is the first check the best-matching face failed. A file of another family, style
/// or PostScript name fails even when its bytes have the index's sha256, as they do if the index
/// itself is wrong, so a document made with one font can't silently render with a lookalike.
pub fn verify_face(family: &Family, file: &FontFile, bytes: &[u8]) -> Result<(), VerifyError> {
    verify_font(file, bytes)?;
    let Ok(parsed) = FileRef::new(bytes) else { return Err(VerifyError::NotAFont) };
    let mut worst: Option<(u8, VerifyError)> = None;
    for font in parsed.fonts() {
        let Ok(font) = font else { return Err(VerifyError::NotAFont) };
        match check_face(family, file, &font) {
            Ok(()) => return Ok(()),
            Err((stage, e)) => {
                if worst.as_ref().is_none_or(|(s, _)| stage > *s) {
                    worst = Some((stage, e));
                }
            }
        }
    }
    Err(worst.map_or(VerifyError::NotAFont, |(_, e)| e))
}

/// Name records read per name ID; a hostile `name` table can repeat one tens of thousands of times.
const MAX_NAME_RECORDS: usize = 512;
/// How closely a width percentage must agree.
const WIDTH_TOLERANCE: f32 = 0.5;
/// How closely an axis range must agree.
const AXIS_TOLERANCE: f32 = 0.01;

fn names(font: &FontRef<'_>, id: StringId) -> Vec<String> {
    font.localized_strings(id).take(MAX_NAME_RECORDS).map(|s| s.to_string()).filter(|s| !s.is_empty()).collect()
}

/// `usWidthClass` (1 to 9) as a percentage of normal.
fn width_percent(class: u16) -> f32 {
    const PERCENT: [f32; 9] = [50.0, 62.5, 75.0, 87.5, 100.0, 112.5, 125.0, 150.0, 200.0];
    usize::from(class).checked_sub(1).and_then(|i| PERCENT.get(i)).copied().unwrap_or(100.0)
}

/// One face against the entry: the stage it failed at (a higher stage matched more) and why.
fn check_face(family: &Family, file: &FontFile, font: &FontRef<'_>) -> Result<(), (u8, VerifyError)> {
    let typographic = names(font, StringId::TYPOGRAPHIC_FAMILY_NAME);
    let found = if typographic.is_empty() { names(font, StringId::FAMILY_NAME) } else { typographic };
    let wanted = family.family.to_lowercase();
    if !found.iter().any(|n| n.to_lowercase() == wanted) {
        let found = found.first().cloned().unwrap_or_else(|| "(none)".into());
        return Err((0, VerifyError::Family { expected: family.family.clone(), found }));
    }

    let os2 = font.os2().ok();
    let weight = os2.as_ref().map_or(400, |t| t.us_weight_class());
    let width = os2.as_ref().map_or(100.0, |t| width_percent(t.us_width_class()));
    let italic = os2.as_ref().is_some_and(|t| t.fs_selection().intersects(SelectionFlags::ITALIC | SelectionFlags::OBLIQUE));
    let axes = font.axes();
    let ital_axis = file.axes.iter().any(|a| a.tag == "ital");
    let describe = |weight: u16, width: f32, italic: bool| format!("weight {weight}, {}, width {width}%", if italic { "italic" } else { "upright" });
    if weight != file.weight || (width - file.width).abs() > WIDTH_TOLERANCE || (!ital_axis && italic != file.is_italic()) {
        let expected = describe(file.weight, file.width, file.is_italic());
        return Err((1, VerifyError::Style { expected, found: describe(weight, width, italic) }));
    }
    if axes.is_empty() != file.axes.is_empty() {
        let found = if axes.is_empty() { "a static font" } else { "a variable font" };
        let expected = if file.axes.is_empty() { "a static font" } else { "a variable font" };
        return Err((1, VerifyError::Style { expected: expected.into(), found: found.into() }));
    }
    for want in &file.axes {
        let have = axes.iter().find(|a| a.tag().to_be_bytes() == want.tag.as_bytes());
        if !have.as_ref().is_some_and(|a| a.min_value() <= want.min + AXIS_TOLERANCE && a.max_value() >= want.max - AXIS_TOLERANCE) {
            let found = have.map_or_else(|| "none".to_string(), |a| format!("{}..{}", a.min_value(), a.max_value()));
            return Err((1, VerifyError::Axis { tag: want.tag.clone(), expected: format!("{}..{}", want.min, want.max), found }));
        }
    }

    if !file.postscript.is_empty() {
        let have = names(font, StringId::POSTSCRIPT_NAME);
        if !have.iter().any(|n| file.postscript.contains(n)) {
            let found = have.first().cloned().unwrap_or_else(|| "(none)".into());
            return Err((2, VerifyError::PostScript { expected: file.postscript.clone(), found }));
        }
    }
    Ok(())
}
