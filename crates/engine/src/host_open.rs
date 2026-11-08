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
    let Some(&first) = imported.sequences.first() else {
        return Err(Refusal::NotRoundTrip { name: name.to_string(), reason: "holds no timeline".into() });
    };
    for item in imported.project.items.values() {
        if let ItemKind::Media(m) = &item.kind
            && let MediaRef::File { path } = &m.media
            && !is_oneos_media(path)
        {
            return Err(Refusal::MediaNotReferenced { name: name.to_string(), media: item.name.clone() });
        }
    }
    // The whole project, as Film writes it (files/989: a SerializableCollection.1 of every sequence, the project under metadata.filmcraft).
    let (again, _) = filmcraft_interchange::export_project(&imported.project, &ExportOptions::default()).map_err(|e| unreadable(e.to_string()))?;
    let again: Value = serde_json::from_slice(&again).map_err(|e| unreadable(e.to_string()))?;
    if let Some(at) = first_difference(&original, &again, "") {
        return Err(Refusal::NotRoundTrip { name: name.to_string(), reason: format!("would be written back differently at {at}") });
    }
    crate::commands::install_project(s, imported.project, Some(name.to_string()), true);
    s.state.active_sequence = Some(first);
    s.state.open_sequences = vec![first].into_iter().collect();
    Ok(())
}

/// The bytes of the whole project as the host stores them (`.otio`); the session is clean after.
pub fn save_host_file(s: &mut Session) -> Result<Vec<u8>, String> {
    let (bytes, _) = filmcraft_interchange::export_project(&s.project, &ExportOptions::default()).map_err(|e| e.to_string())?;
    s.saved_revision = s.revision;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(std::sync::Arc::new(crate::FsServices))
    }

    #[test]
    fn anything_but_an_otio_is_refused_by_name() {
        let mut s = session();
        let e = open_host_file(&mut s, "cut.fcproj", b"{}").unwrap_err();
        assert!(e.to_string().starts_with("format-not-editable:"), "{e}");
        let e = open_host_file(&mut s, "x.otio", b"not json").unwrap_err();
        assert!(e.to_string().starts_with("file-unreadable:"), "{e}");
    }

    #[test]
    fn a_file_media_path_is_refused_and_a_oneos_file_opens_and_saves_back_value_for_value() {
        let otio = |url: &str| {
            format!(
                r#"{{"OTIO_SCHEMA":"Timeline.1","name":"T","global_start_time":null,"metadata":{{}},"tracks":{{"OTIO_SCHEMA":"Stack.1","name":"tracks","children":[{{"OTIO_SCHEMA":"Track.1","name":"V1","kind":"Video","children":[{{"OTIO_SCHEMA":"Clip.2","name":"beach","source_range":{{"OTIO_SCHEMA":"TimeRange.1","start_time":{{"OTIO_SCHEMA":"RationalTime.1","rate":24,"value":0}},"duration":{{"OTIO_SCHEMA":"RationalTime.1","rate":24,"value":48}}}},"media_references":{{"DEFAULT_MEDIA":{{"OTIO_SCHEMA":"ExternalReference.1","name":"beach","target_url":"{url}","available_range":null,"metadata":{{}}}}}},"active_media_reference_key":"DEFAULT_MEDIA"}}]}}]}}}}"#
            )
        };
        let mut s = session();
        let e = open_host_file(&mut s, "a.otio", otio("file:///m/beach.mov").as_bytes()).unwrap_err();
        // A plain OTIO from another tool is first refused for not being in Film's form; a path is refused by name in Film's own form.
        assert!(e.to_string().starts_with("format-not-round-trip:") || e.to_string().starts_with("media-not-referenced:"), "{e}");
        // Film's own form of a timeline with a OneOS medium: read the plain one, write the whole project, open that.
        let (imp, _) = filmcraft_interchange::import(otio("oneos:file/01jabcdefghjkmnpqrstvwxyz0?version=v3").as_bytes(), Format::Otio, None).unwrap();
        let (film_form, _) = filmcraft_interchange::export_project(&imp.project, &ExportOptions::default()).unwrap();
        open_host_file(&mut s, "a.otio", &film_form).unwrap();
        assert!(!s.is_dirty());
        let saved = save_host_file(&mut s).unwrap();
        let (a, b): (Value, Value) = (serde_json::from_slice(&film_form).unwrap(), serde_json::from_slice(&saved).unwrap());
        assert_eq!(first_difference(&a, &b, ""), None, "saved back value for value");
        assert!(String::from_utf8_lossy(&saved).contains("oneos:file/01jabcdefghjkmnpqrstvwxyz0?version=v3"));
    }
}
