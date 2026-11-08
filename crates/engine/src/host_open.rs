//! The file the OneOS host opens in Film, and the bytes a save hands back (OneOS doctrine files/989).
//!
//! Film edits ONE format for the host: OpenTimelineIO (`.otio`, Mona 2026-11-08: "OpenTimelineIO .otio"), the video project format Film
//! and Cut share. A timeline opens only when Film can write it back UNCHANGED, value for value: the interchange reads it, writes it again,
//! and any difference (a part Film cannot represent, or represents another way) refuses the file by name, naming the first place it
//! differs, so a save can never drop or rewrite it. Every medium must be a OneOS file (`oneos:file/<id>?version=<v>`); a path or an
//! address is refused. Anything else (a `.fcproj`, a picture, a movie) is an import or an export, never a document a save writes back.

use filmcraft_interchange::{ExportOptions, Format, is_oneos_media};
use filmcraft_project::{ItemKind, MediaRef};
use serde_json::Value;

use crate::Session;

/// The extension Film edits for the host.
pub const EDITS: &[&str] = &["otio"];

/// Why a file is not opened for editing, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    NotEditable(String),
    Unreadable { name: String, error: String },
    NotRoundTrip { name: String, reason: String },
    MediaNotReferenced { name: String, media: String },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEditable(name) => write!(f, "format-not-editable: Film edits OpenTimelineIO timelines (.otio); {name} is not opened for editing"),
            Self::Unreadable { name, error } => write!(f, "file-unreadable: {name} could not be read as an OpenTimelineIO timeline: {error}"),
            Self::NotRoundTrip { name, reason } => write!(f, "format-not-round-trip: {name} {reason}, so a save would change it; it is not opened for editing"),
            Self::MediaNotReferenced { name, media } => write!(f, "media-not-referenced: {name} names the medium {media}, which is not a OneOS file and version"),
        }
    }
}

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// The JSON pointer of the first place two documents differ (`""` when they are the same).
fn first_difference(a: &Value, b: &Value, at: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            keys.into_iter().find_map(|k| match (x.get(k), y.get(k)) {
                (Some(p), Some(q)) => first_difference(p, q, &format!("{at}/{k}")),
                _ => Some(format!("{at}/{k}")),
            })
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x.iter().zip(y).enumerate().find_map(|(i, (p, q))| first_difference(p, q, &format!("{at}/{i}"))),
        (Value::Number(x), Value::Number(y)) if x.as_f64() == y.as_f64() => None,
        _ if a == b => None,
        _ => Some(if at.is_empty() { "/".into() } else { at.into() }),
    }
}

/// Open what the host sent into `s`, or refuse it by name (nothing is changed then).
pub fn open_host_file(s: &mut Session, name: &str, bytes: &[u8]) -> Result<(), Refusal> {
    if !EDITS.contains(&extension(name).as_str()) {
        return Err(Refusal::NotEditable(name.to_string()));
    }
    let unreadable = |e: String| Refusal::Unreadable { name: name.to_string(), error: e };
    let original: Value = serde_json::from_slice(bytes).map_err(|e| unreadable(e.to_string()))?;
    let (imported, report) = filmcraft_interchange::import(bytes, Format::Otio, None).map_err(|e| unreadable(e.to_string()))?;
    if report.has_warnings() {
        let said: Vec<String> = report.warnings().map(|w| w.message.clone()).collect();
        return Err(Refusal::NotRoundTrip { name: name.to_string(), reason: format!("holds what Film cannot represent ({})", said.join("; ")) });
    }
    let [seq] = imported.sequences[..] else {
        return Err(Refusal::NotRoundTrip { name: name.to_string(), reason: format!("holds {} timelines; Film opens one", imported.sequences.len()) });
    };
    for item in imported.project.items.values() {
        if let ItemKind::Media(m) = &item.kind
            && let MediaRef::File { path } = &m.media
            && !is_oneos_media(path)
        {
            return Err(Refusal::MediaNotReferenced { name: name.to_string(), media: item.name.clone() });
        }
    }
    let (again, _) = filmcraft_interchange::export(&imported.project, seq, Format::Otio, &ExportOptions::default()).map_err(|e| unreadable(e.to_string()))?;
    let again: Value = serde_json::from_slice(&again).map_err(|e| unreadable(e.to_string()))?;
    if let Some(at) = first_difference(&original, &again, "") {
        return Err(Refusal::NotRoundTrip { name: name.to_string(), reason: format!("would be written back differently at {at}") });
    }
    crate::commands::install_project(s, imported.project, Some(name.to_string()), true);
    s.state.active_sequence = Some(seq);
    s.state.open_sequences = vec![seq].into_iter().collect();
    Ok(())
}

/// The bytes of the open timeline as the host stores them (`.otio`); the session is clean after.
pub fn save_host_file(s: &mut Session) -> Result<Vec<u8>, String> {
    let seq = s.state.active_sequence.ok_or("no-timeline: no timeline is open")?;
    let (bytes, _) = filmcraft_interchange::export(&s.project, seq, Format::Otio, &ExportOptions::default()).map_err(|e| e.to_string())?;
    s.saved_revision = s.revision;
    Ok(bytes)
}
