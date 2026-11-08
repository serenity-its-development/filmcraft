//! The file the OneOS host opens in Film, and the bytes a save hands back (OneOS doctrine files/989).
//!
//! Film edits ONE format for the host: OpenTimelineIO (`.otio`, Mona 2026-11-08: "OpenTimelineIO .otio"), the video project format Film
//! and Cut share. A timeline opens only when Film can write it back UNCHANGED, value for value: the interchange reads it, writes it again,
//! and any difference (a part Film cannot represent, or represents another way) refuses the file by name, naming the first place it
//! differs, so a save can never drop or rewrite it. Every medium must be a OneOS file (`oneos:file/<id>?version=<v>`); a path or an
//! address is refused. Anything else (a `.fcproj`, a picture, a movie) is an import or an export, never a document a save writes back.
//!
//! One format holds everything of both editors (Mona 2026-11-08: "one format that can encompass everything of both"): Film CARRIES Cut's
//! own parts of a shared file untouched, the way Cut carries Film's. Before Film reads a file it sets aside Cut's timelines (a `Timeline.1`
//! with `metadata.opencut.scene`), every track holding Cut's data (any `opencut` key within it: Cut's text, sticker, graphic and effect
//! clips, its stacked lanes, a track Cut edited), Cut's bookmarks on a stack and the root's metadata keys other than Film's; Film reads and
//! edits the rest, and a save puts what it set aside back in its place. What is set aside is listed by name (`Carried::named`): it is shown,
//! not edited, in Film.

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

/// What Film carries untouched through a shared `.otio`: Cut's own parts, set aside before Film reads the rest and put back in their places
/// on save (files/989).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Carried {
    /// The root's metadata keys other than Film's own (`filmcraft`).
    pub root_metadata: serde_json::Map<String, Value>,
    /// Cut's own timelines, each at its index among the collection's children.
    pub timelines: Vec<(usize, Value)>,
    /// Tracks holding Cut's data: (the index of Film's timeline among Film's timelines, the index in its stack, the track).
    pub tracks: Vec<(usize, usize, Value)>,
    /// Cut's bookmarks on the stack of Film's timeline (by its index), after Film's own markers.
    pub markers: Vec<(usize, Vec<Value>)>,
}

impl Carried {
    /// What Film shows but does not edit, by name.
    pub fn named(&self) -> Vec<String> {
        let name = |v: &Value| v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let mut out: Vec<String> = self.timelines.iter().map(|(_, t)| format!("the timeline \"{}\" (Cut's)", name(t))).collect();
        out.extend(self.tracks.iter().map(|(_, _, t)| format!("the track \"{}\" (holds Cut's edits)", name(t))));
        out
    }
}

/// Does `v` hold Cut's data (an `opencut` key anywhere within it)?
fn holds_cut(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.contains_key("opencut") || m.values().any(holds_cut),
        Value::Array(a) => a.iter().any(holds_cut),
        _ => false,
    }
}

fn is_timeline(v: &Value) -> bool {
    v.get("OTIO_SCHEMA").and_then(Value::as_str) == Some("Timeline.1")
}

/// Film's part of `doc` and what it carries.
pub fn set_aside(doc: &Value) -> (Value, Carried) {
    let mut film = doc.clone();
    let mut c = Carried::default();
    if film.get("OTIO_SCHEMA").and_then(Value::as_str) == Some("SerializableCollection.1") {
        if let Some(Value::Object(meta)) = film.get_mut("metadata") {
            let other: Vec<String> = meta.keys().filter(|k| k.as_str() != "filmcraft").cloned().collect();
            for k in other {
                if let Some(v) = meta.remove(&k) {
                    c.root_metadata.insert(k, v);
                }
            }
        }
        if let Some(Value::Array(children)) = film.get_mut("children") {
            let kept: Vec<Value> = std::mem::take(children)
                .into_iter()
                .enumerate()
                .filter_map(|(i, t)| {
                    if is_timeline(&t) && t.pointer("/metadata/opencut/scene").is_some_and(Value::is_object) {
                        c.timelines.push((i, t));
                        None
                    } else {
                        Some(t)
                    }
                })
                .collect();
            *children = kept;
        }
    }
    for (fi, tl) in film_timelines(&mut film).into_iter().enumerate() {
        let Some(Value::Object(stack)) = tl.get_mut("tracks") else { continue };
        if let Some(Value::Array(tracks)) = stack.get_mut("children") {
            let kept: Vec<Value> = std::mem::take(tracks)
                .into_iter()
                .enumerate()
                .filter_map(|(k, t)| {
                    if holds_cut(&t) {
                        c.tracks.push((fi, k, t));
                        None
                    } else {
                        Some(t)
                    }
                })
                .collect();
            *tracks = kept;
        }
        if let Some(Value::Array(markers)) = stack.get_mut("markers") {
            let (cut, own): (Vec<Value>, Vec<Value>) = std::mem::take(markers).into_iter().partition(holds_cut);
            *markers = own;
            if !cut.is_empty() {
                c.markers.push((fi, cut));
            }
        }
    }
    (film, c)
}

/// Film's timelines in `doc` (the root itself when it is one).
fn film_timelines(doc: &mut Value) -> Vec<&mut Value> {
    if is_timeline(doc) {
        return vec![doc];
    }
    match doc.get_mut("children") {
        Some(Value::Array(children)) => children.iter_mut().filter(|t| is_timeline(t)).collect(),
        _ => Vec::new(),
    }
}

/// Film's written part with what it carried put back in place.
pub fn put_back(mut film: Value, c: &Carried) -> Value {
    if !c.root_metadata.is_empty()
        && let Some(root) = film.as_object_mut()
    {
        let meta = root.entry("metadata").or_insert_with(|| Value::Object(Default::default()));
        if let Value::Object(m) = meta {
            for (k, v) in &c.root_metadata {
                m.insert(k.clone(), v.clone());
            }
        }
    }
    let mut timelines = film_timelines(&mut film);
    for (fi, k, t) in &c.tracks {
        if let Some(Some(Value::Array(tracks))) = timelines.get_mut(*fi).map(|tl| tl.pointer_mut("/tracks/children")) {
            let at = (*k).min(tracks.len());
            tracks.insert(at, t.clone());
        }
    }
    for (fi, ms) in &c.markers {
        if let Some(Some(stack)) = timelines.get_mut(*fi).map(|tl| tl.pointer_mut("/tracks"))
            && let Some(obj) = stack.as_object_mut()
        {
            let markers = obj.entry("markers").or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(a) = markers {
                a.extend(ms.iter().cloned());
            }
        }
    }
    if !c.timelines.is_empty()
        && let Some(Value::Array(children)) = film.get_mut("children")
    {
        for (i, t) in &c.timelines {
            let at = (*i).min(children.len());
            children.insert(at, t.clone());
        }
    }
    film
}

/// Open what the host sent into `s`, or refuse it by name (nothing is changed then).
pub fn open_host_file(s: &mut Session, name: &str, bytes: &[u8]) -> Result<(), Refusal> {
    if !EDITS.contains(&extension(name).as_str()) {
        return Err(Refusal::NotEditable(name.to_string()));
    }
    let unreadable = |e: String| Refusal::Unreadable { name: name.to_string(), error: e };
    let original: Value = serde_json::from_slice(bytes).map_err(|e| unreadable(e.to_string()))?;
    let (film_part, carried) = set_aside(&original);
    let film_bytes = serde_json::to_vec(&film_part).map_err(|e| unreadable(e.to_string()))?;
    let (imported, report) = filmcraft_interchange::import(&film_bytes, Format::Otio, None).map_err(|e| unreadable(e.to_string()))?;
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
    let again = put_back(serde_json::from_slice(&again).map_err(|e| unreadable(e.to_string()))?, &carried);
    if let Some(at) = first_difference(&original, &again, "") {
        return Err(Refusal::NotRoundTrip { name: name.to_string(), reason: format!("would be written back differently at {at}") });
    }
    crate::commands::install_project(s, imported.project, Some(name.to_string()), true);
    s.host_carried = carried;
    s.state.active_sequence = Some(first);
    s.state.open_sequences = vec![first].into_iter().collect();
    Ok(())
}

/// The bytes of the whole project as the host stores them (`.otio`); the session is clean after.
pub fn save_host_file(s: &mut Session) -> Result<Vec<u8>, String> {
    let (bytes, _) = filmcraft_interchange::export_project(&s.project, &ExportOptions::default()).map_err(|e| e.to_string())?;
    let film: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let whole = serde_json::to_vec_pretty(&put_back(film, &s.host_carried)).map_err(|e| e.to_string())?;
    s.saved_revision = s.revision;
    Ok(whole)
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

    /// Film's own form of a one-clip timeline whose medium is a OneOS file.
    fn film_form() -> Value {
        let plain = r#"{"OTIO_SCHEMA":"Timeline.1","name":"T","global_start_time":null,"metadata":{},"tracks":{"OTIO_SCHEMA":"Stack.1","name":"tracks","children":[{"OTIO_SCHEMA":"Track.1","name":"V1","kind":"Video","children":[{"OTIO_SCHEMA":"Clip.2","name":"beach","source_range":{"OTIO_SCHEMA":"TimeRange.1","start_time":{"OTIO_SCHEMA":"RationalTime.1","rate":24,"value":0},"duration":{"OTIO_SCHEMA":"RationalTime.1","rate":24,"value":48}},"media_references":{"DEFAULT_MEDIA":{"OTIO_SCHEMA":"ExternalReference.1","name":"beach","target_url":"oneos:file/01jabcdefghjkmnpqrstvwxyz0?version=v3","available_range":null,"metadata":{}}},"active_media_reference_key":"DEFAULT_MEDIA"}]}]}}"#;
        let (imp, _) = filmcraft_interchange::import(plain.as_bytes(), Format::Otio, None).unwrap();
        let (bytes, _) = filmcraft_interchange::export_project(&imp.project, &ExportOptions::default()).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// What Cut writes into a Film file (OpenCut fork `apps/web/src/oneos/cut-session.ts`): a text track, its stacked lanes, a scene of its
    /// own, a bookmark on Film's stack and its settings on the root.
    fn with_cut_parts(mut v: Value) -> Value {
        use serde_json::json;
        let rt = |value: i64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": 120000, "value": value});
        let range = |s: i64, d: i64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(s), "duration": rt(d)});
        let text = |name: &str, s: i64| json!({"OTIO_SCHEMA": "Clip.2", "name": name, "source_range": range(0, 120000), "effects": [], "markers": [], "enabled": true,
            "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "GeneratorReference.1", "name": name, "generator_kind": "OpenCut.text", "parameters": {}, "available_range": null, "metadata": {}}},
            "active_media_reference_key": "DEFAULT_MEDIA", "metadata": {"opencut": {"element": {"content": "Hello", "fontSize": 64, "trimEnd": 0, "start": s}}}});
        let gap = |d: i64| json!({"OTIO_SCHEMA": "Gap.1", "name": "", "source_range": range(0, d), "effects": [], "markers": [], "enabled": true, "metadata": {}});
        let track = |name: &str, meta: Value, children: Value| json!({"OTIO_SCHEMA": "Track.1", "name": name, "kind": "Video", "source_range": null, "effects": [], "markers": [], "enabled": true, "metadata": meta, "children": children});
        let titles = track("Titles", json!({"opencut": {"track": {"type": "text", "hidden": false}}}), json!([text("Title", 0), gap(120000), text("End", 2)]));
        let lanes = json!({"OTIO_SCHEMA": "Stack.1", "name": "Stickers", "source_range": null, "effects": [], "markers": [], "enabled": true, "metadata": {"opencut": {"track": {"type": "graphic", "hidden": false}}},
            "children": [track("Stickers (lane 1)", json!({"opencut": {"lane": 0}}), json!([text("a", 0)])), track("Stickers (lane 2)", json!({"opencut": {"lane": 1}}), json!([text("b", 1)]))]});
        let tracks = v.pointer_mut("/children/0/tracks/children").and_then(Value::as_array_mut).unwrap();
        tracks.push(titles.clone());
        tracks.push(lanes);
        let markers = v.pointer_mut("/children/0/tracks").and_then(Value::as_object_mut).unwrap().entry("markers").or_insert_with(|| json!([]));
        markers.as_array_mut().unwrap().push(json!({"OTIO_SCHEMA": "Marker.2", "name": "hook", "marked_range": range(120000, 0), "color": "RED", "comment": "", "metadata": {"opencut": {"bookmark": {"note": "hook"}}}}));
        let teaser = json!({"OTIO_SCHEMA": "Timeline.1", "name": "Teaser", "global_start_time": null, "metadata": {"opencut": {"scene": {"isMain": false}}},
            "tracks": {"OTIO_SCHEMA": "Stack.1", "name": "tracks", "source_range": null, "effects": [], "markers": [], "enabled": true, "metadata": {}, "children": [titles]}});
        v["children"].as_array_mut().unwrap().push(teaser);
        v["metadata"]["opencut"] = json!({"settings": {"fps": {"numerator": 30, "denominator": 1}, "canvasSize": {"width": 1080, "height": 1920}}});
        v
    }

    /// files/989 ("one format that can encompass everything of both"): Cut -> Film -> Cut. Film opens a file Cut wrote its own parts into,
    /// carries them untouched (named, shown, not edited), and a save, unedited or edited, gives every one of them back exactly.
    #[test]
    fn cut_parts_in_a_film_file_are_carried_through_film_untouched() {
        let cut = with_cut_parts(film_form());
        let bytes = serde_json::to_vec(&cut).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", &bytes).unwrap();
        assert!(!s.is_dirty());
        assert_eq!(s.host_carried.named(), vec!["the timeline \"Teaser\" (Cut's)".to_string(), "the track \"Titles\" (holds Cut's edits)".into(), "the track \"Stickers\" (holds Cut's edits)".into()]);
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "unedited: Cut's file comes back value for value");
        // Film edits its own part; every part of Cut's is still exactly what Cut wrote.
        std::sync::Arc::make_mut(&mut s.project).notes = "Film was here".into();
        let edited: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_ne!(first_difference(&cut, &edited, ""), None, "Film's edit is saved");
        for at in ["/children/0/tracks/children/1", "/children/0/tracks/children/2", "/children/1", "/metadata/opencut"] {
            assert_eq!(edited.pointer(at), cut.pointer(at), "{at} is Cut's, untouched");
        }
        let marks = |v: &Value| v.pointer("/children/0/tracks/markers").cloned();
        assert_eq!(marks(&edited), marks(&cut));
        assert_eq!(edited.pointer("/children/0/tracks/children/0"), cut.pointer("/children/0/tracks/children/0"), "Film's own track");
    }

    #[test]
    fn set_aside_and_put_back_are_inverse_and_take_nothing_of_film_s_own() {
        let film = film_form();
        let (part, c) = set_aside(&film);
        assert_eq!(c, Carried::default(), "a Film file carries nothing");
        assert_eq!(part, film);
        let cut = with_cut_parts(film_form());
        let (part, c) = set_aside(&cut);
        assert_eq!(part, film, "what is set aside is exactly Cut's");
        assert_eq!(put_back(part, &c), cut);
    }
}
