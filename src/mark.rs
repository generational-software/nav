//! A folder can carry a `.nav` file that sets the colour and shape of its stone.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Shape {
    Prism = 0,
    Cylinder = 1,
    Box = 2,
    Cone = 3,
    Pyramid = 4,
    Crystal = 5,
    Obelisk = 6,
    /// Round marker for a command on the center ring. Not a choice in `.nav`.
    Blob = 7,
}

impl Shape {
    pub const COUNT: usize = 8;
    /// Shapes a person can put in a `.nav` file.
    pub const STONES: [Shape; 7] = [
        Shape::Prism,
        Shape::Cylinder,
        Shape::Box,
        Shape::Cone,
        Shape::Pyramid,
        Shape::Crystal,
        Shape::Obelisk,
    ];
    pub const ALL: [Shape; Self::COUNT] = [
        Shape::Prism,
        Shape::Cylinder,
        Shape::Box,
        Shape::Cone,
        Shape::Pyramid,
        Shape::Crystal,
        Shape::Obelisk,
        Shape::Blob,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Shape::Prism => "prism",
            Shape::Cylinder => "cylinder",
            Shape::Box => "box",
            Shape::Cone => "cone",
            Shape::Pyramid => "pyramid",
            Shape::Crystal => "crystal",
            Shape::Obelisk => "obelisk",
            Shape::Blob => "blob",
        }
    }

    pub fn parse(text: &str) -> Option<Shape> {
        Self::STONES
            .into_iter()
            .find(|shape| shape.name().eq_ignore_ascii_case(text.trim()))
    }
}

impl Default for Shape {
    fn default() -> Self {
        Shape::Prism
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavMark {
    pub color: Option<[f32; 4]>,
    pub shape: Shape,
}

impl Default for NavMark {
    fn default() -> Self {
        Self {
            color: None,
            shape: Shape::Prism,
        }
    }
}

impl NavMark {
    /// Read `path/.nav`. A missing file, or one this program cannot use, leaves the stone as it was.
    pub fn read(dir: &Path) -> Self {
        let path = dir.join(".nav");
        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };
        match parse_mark(&text) {
            Ok(parsed) => {
                for err in &parsed.errors {
                    eprintln!("nav: {}: {err}", path.display());
                }
                parsed.mark
            }
            Err(err) => {
                eprintln!("nav: {}: {err}", path.display());
                Self::default()
            }
        }
    }
}

struct ParsedMark {
    mark: NavMark,
    errors: Vec<String>,
}

const COLOURS: &[(&str, [f32; 3])] = &[
    ("amber", [0.86, 0.56, 0.26]),
    ("teal", [0.20, 0.62, 0.66]),
    ("red", [0.80, 0.24, 0.22]),
    ("green", [0.28, 0.62, 0.34]),
    ("blue", [0.26, 0.42, 0.78]),
    ("yellow", [0.86, 0.74, 0.24]),
    ("orange", [0.88, 0.46, 0.16]),
    ("purple", [0.52, 0.32, 0.72]),
    ("pink", [0.82, 0.40, 0.56]),
    ("white", [0.90, 0.92, 0.90]),
    ("black", [0.12, 0.13, 0.15]),
    ("grey", [0.55, 0.58, 0.60]),
    ("gray", [0.55, 0.58, 0.60]),
    ("brown", [0.48, 0.32, 0.20]),
    ("gold", [0.86, 0.70, 0.28]),
    ("cyan", [0.28, 0.74, 0.78]),
    ("lime", [0.52, 0.74, 0.26]),
    ("navy", [0.16, 0.26, 0.48]),
    ("violet", [0.48, 0.28, 0.70]),
    ("coral", [0.88, 0.42, 0.36]),
    ("mint", [0.40, 0.74, 0.62]),
    ("sand", [0.76, 0.66, 0.46]),
    ("copper", [0.72, 0.42, 0.26]),
    ("rose", [0.78, 0.36, 0.46]),
];

pub fn colour_names() -> String {
    COLOURS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")
}

pub fn shape_names() -> String {
    Shape::STONES.iter().map(|shape| shape.name()).collect::<Vec<_>>().join(", ")
}

pub fn parse_colour(text: &str) -> Option<[f32; 4]> {
    let text = text.trim();
    if let Some((_, rgb)) = COLOURS.iter().find(|(name, _)| name.eq_ignore_ascii_case(text)) {
        return Some([rgb[0], rgb[1], rgb[2], 1.0]);
    }
    parse_hex(text)
}

/// Write `.nav` in `dir`. The first value is the colour and the second is the shape.
/// A file that is already a JSON object keeps its other keys.
pub fn write_tag(dir: &Path, colour: &str, shape: &str) -> Result<(), String> {
    let colour_text = canonical_colour(colour).ok_or_else(|| {
        format!("unknown colour \"{colour}\". colours: {}", colour_names())
    })?;
    let shape = Shape::parse(shape).ok_or_else(|| {
        format!("unknown shape \"{shape}\". shapes: {}", shape_names())
    })?;
    let path = dir.join(".nav");
    let mut object = if path.is_file() {
        let text = fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map,
            Ok(_) => return Err(format!("{} is not a JSON object", path.display())),
            Err(err) => return Err(format!("{}: {err}", path.display())),
        }
    } else {
        Map::new()
    };
    object.insert("colour".to_string(), Value::String(colour_text));
    object.insert("shape".to_string(), Value::String(shape.name().to_string()));
    let body = serde_json::to_string_pretty(&Value::Object(object))
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    fs::write(&path, body + "\n").map_err(|err| format!("{}: {err}", path.display()))
}

fn canonical_colour(text: &str) -> Option<String> {
    let text = text.trim();
    if let Some((name, _)) = COLOURS.iter().find(|(name, _)| name.eq_ignore_ascii_case(text)) {
        return Some((*name).to_string());
    }
    let rgba = parse_hex(text)?;
    Some(format_hex(rgba))
}

fn parse_mark(text: &str) -> Result<ParsedMark, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let Some(object) = value.as_object() else {
        return Err("not a JSON object".into());
    };
    let mut mark = NavMark::default();
    let mut errors = Vec::new();
    if let Some(colour) = object.get("colour").and_then(Value::as_str) {
        match parse_colour(colour) {
            Some(color) => mark.color = Some(color),
            None => errors.push(format!("unknown colour \"{colour}\"")),
        }
    }
    if let Some(shape) = object.get("shape").and_then(Value::as_str) {
        match Shape::parse(shape) {
            Some(shape) => mark.shape = shape,
            None => errors.push(format!("unknown shape \"{shape}\"")),
        }
    }
    Ok(ParsedMark { mark, errors })
}

fn parse_hex(text: &str) -> Option<[f32; 4]> {
    let text = text.trim().trim_start_matches('#');
    if text.is_empty() || !text.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let expanded: String = match text.len() {
        3 | 4 => text.chars().flat_map(|ch| [ch, ch]).collect(),
        6 | 8 => text.to_string(),
        _ => return None,
    };
    let mut bytes = [255u8; 4];
    for (i, chunk) in expanded.as_bytes().chunks(2).enumerate() {
        let digits = std::str::from_utf8(chunk).ok()?;
        bytes[i] = u8::from_str_radix(digits, 16).ok()?;
    }
    Some([
        bytes[0] as f32 / 255.0,
        bytes[1] as f32 / 255.0,
        bytes[2] as f32 / 255.0,
        bytes[3] as f32 / 255.0,
    ])
}

fn format_hex(rgba: [f32; 4]) -> String {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (r, g, b, a) = (byte(rgba[0]), byte(rgba[1]), byte(rgba[2]), byte(rgba[3]));
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_reads_colour_and_shape_and_ignores_a_missing_file() {
        let parsed = parse_mark(r##"{ "colour": "#fc0", "shape": "Cone", "note": 1 }"##).unwrap();
        let gold = parse_colour("#ffcc00").unwrap();
        assert!((parsed.mark.color.unwrap()[0] - gold[0]).abs() < 0.01);
        assert_eq!(parsed.mark.shape, Shape::Cone);
        assert!(parsed.errors.is_empty());
        let empty = parse_mark("{}").unwrap();
        assert!(empty.mark.color.is_none());
        assert_eq!(empty.mark.shape, Shape::Prism);
        assert!(parse_mark("[]").is_err());
        let bad = parse_mark(r#"{ "colour": "nope", "shape": "cone" }"#).unwrap();
        assert!(bad.mark.color.is_none());
        assert_eq!(bad.mark.shape, Shape::Cone);
        assert_eq!(bad.errors.len(), 1);
        let missing = std::env::temp_dir().join(format!("nav-missing-{}-", std::process::id()));
        assert_eq!(NavMark::read(&missing), NavMark::default());
    }

    #[test]
    fn tag_writes_colour_and_shape_and_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!(
            "nav-tag-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".nav"), "{\n  \"note\": \"keep\",\n  \"colour\": \"teal\"\n}\n").unwrap();
        write_tag(&dir, "RED", "obelisk").unwrap();
        let text = fs::read_to_string(dir.join(".nav")).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["note"], "keep");
        assert_eq!(value["colour"], "red");
        assert_eq!(value["shape"], "obelisk");
        let mark = NavMark::read(&dir);
        assert_eq!(mark.shape, Shape::Obelisk);
        assert_eq!(mark.color, parse_colour("red"));
        assert!(write_tag(&dir, "nope", "cone").is_err());
        assert!(write_tag(&dir, "red", "orb").is_err());
        assert!(write_tag(&dir, "red", "blob").is_err());
        assert!(Shape::parse("blob").is_none());
        assert!(!shape_names().split(", ").any(|name| name == "blob"));
        let _ = fs::remove_dir_all(&dir);
    }
}
