//! The file the OneOS host opens in Film, and the bytes a save hands back (OneOS doctrine files/989).
//!
//! Film edits ONE format for the host: OpenTimelineIO (`.otio`, Mona 2026-11-08: "OpenTimelineIO .otio"), the video project format Film
//! and Cut share, as two tools on one file (Mona 2026-11-09: "yes, so this becomes a tool choice not a file type choicer"). Film opens ALL
//! of it as its own types and edits it (Mona 2026-11-09: "film should be able to edit cut though"): its own timelines through the project it
//! keeps (a timeline Cut changed is reconciled into it), Cut's scenes, tracks, elements and bookmarks as Film sequences, tracks, clips and
//! markers, written back in Cut's shape so Cut reopens them editable (`filmcraft_interchange::otio::oneos`). A save is a three-way merge:
//! what Film did not change is written exactly as the file held it. Every medium must be a OneOS file (`oneos:file/<id>?version=<v>`); a
//! path or an address is refused. A part Film would damage is LOCKED (shown, carried untouched) and named; a file Film would damage is
//! refused by name. Anything but an `.otio` (a `.fcproj`, a picture, a movie) is an import or an export, never a document a save writes back.

use filmcraft_interchange::otio::oneos::{SharedError, open_shared, save_shared};

use crate::Session;

/// The extension Film edits for the host.
pub const EDITS: &[&str] = &["otio"];

/// Why a file is not opened (or saved) for editing, by name.
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

/// Open what the host sent into `s`, or refuse it by name (nothing is changed then).
pub fn open_host_file(s: &mut Session, name: &str, bytes: &[u8]) -> Result<(), Refusal> {
    if !EDITS.contains(&extension(name).as_str()) {
        return Err(Refusal::NotEditable(name.to_string()));
    }
    let (project, first, shared) = open_shared(name, bytes).map_err(|e| match e {
        SharedError::Unreadable(error) => Refusal::Unreadable { name: name.to_string(), error },
        SharedError::NotRoundTrip(reason) => Refusal::NotRoundTrip { name: name.to_string(), reason },
        SharedError::MediaNotReferenced(media) => Refusal::MediaNotReferenced { name: name.to_string(), media },
    })?;
    let Some(first) = first else {
        return Err(Refusal::NotRoundTrip { name: name.to_string(), reason: "holds no timeline".into() });
    };
    crate::commands::install_project(s, project, Some(name.to_string()), true);
    s.host_shared = shared;
    s.state.active_sequence = Some(first);
    s.state.open_sequences = vec![first].into_iter().collect();
    Ok(())
}

/// The bytes of the whole file as the host stores them (`.otio`), or a refusal by name (nothing is written then); the session is clean after.
pub fn save_host_file(s: &mut Session) -> Result<Vec<u8>, String> {
    let bytes = save_shared(&s.project, &mut s.host_shared).map_err(|e| e.to_string())?;
    s.saved_revision = s.revision;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_interchange::{ExportOptions, Format};
    use filmcraft_project::{ParamValue, TrackKind};
    use serde_json::Value;

    fn session() -> Session {
        Session::new(std::sync::Arc::new(crate::FsServices))
    }

    /// The JSON pointer of the first place two documents differ (`None`: the same values).
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

    /// The sequence named `name`, and the clip named `clip` on it (its track's kind, index and the clip's index).
    fn find(s: &Session, seq: &str, clip: &str) -> (filmcraft_project::ItemId, TrackKind, usize, usize) {
        let it = s.project.items.values().find(|i| i.name == seq && i.as_sequence().is_some()).unwrap_or_else(|| panic!("no sequence {seq}"));
        let q = it.as_sequence().unwrap();
        for kind in [TrackKind::Video, TrackKind::Audio] {
            for (ti, t) in q.tracks(kind).iter().enumerate() {
                if let Some(ci) = t.items.iter().position(|c| c.name == clip) {
                    return (it.id, kind, ti, ci);
                }
            }
        }
        panic!("no clip {clip} on {seq}")
    }

    fn edit_clip(s: &mut Session, seq: &str, clip: &str, f: impl FnOnce(&mut filmcraft_project::TrackItem)) {
        let (id, kind, ti, ci) = find(s, seq, clip);
        let p = std::sync::Arc::make_mut(&mut s.project);
        f(&mut p.sequence_mut(id).unwrap().tracks_mut(kind)[ti].items[ci]);
    }

    fn text_of(c: &filmcraft_project::TrackItem) -> String {
        let l = c.effects.iter().find(|e| e.effect == "graphic_text").expect("a text layer");
        match &l.params["text"].value {
            ParamValue::Text(t) => t.clone(),
            other => panic!("{other:?}"),
        }
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

    /// files/989 (Mona 2026-11-09: "film should be able to edit cut though"): Cut's parts of a Film file open in Film as Film's own types (a
    /// text track as a track of graphic clips with a text layer, Cut's stacked lanes as tracks, its scene as a sequence, its bookmark as a
    /// marker), nothing locked; an unedited save is the file value for value, and so is a save after Film edits only its own part.
    #[test]
    fn cut_parts_in_a_film_file_open_as_film_types_and_save_back_value_for_value() {
        let cut = with_cut_parts(film_form());
        let bytes = serde_json::to_vec(&cut).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", &bytes).unwrap();
        assert!(!s.is_dirty());
        assert_eq!(s.host_shared.locked(), Vec::<String>::new(), "nothing of Cut's is locked");
        let (_, _, _, _) = find(&s, "Teaser", "Title");
        let (seq, kind, ti, ci) = find(&s, "T", "End");
        let q = s.project.sequence(seq).unwrap();
        assert_eq!(kind, TrackKind::Video);
        assert_eq!(q.video_tracks[ti].name, "Titles");
        assert_eq!(text_of(&q.video_tracks[ti].items[ci]), "Hello");
        assert!(q.video_tracks.iter().any(|t| t.name == "Stickers (lane 2)"), "Cut's lanes are tracks");
        assert!(q.markers.iter().any(|m| m.name == "hook"), "Cut's bookmark is a marker");
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "unedited: the file comes back value for value");
        std::sync::Arc::make_mut(&mut s.project).notes = "Film was here".into();
        let edited: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_ne!(first_difference(&cut, &edited, ""), None, "Film's edit is saved");
        for at in ["/children/0/tracks/children/1", "/children/0/tracks/children/2", "/children/0/tracks/markers", "/children/1", "/metadata/opencut"] {
            assert_eq!(edited.pointer(at), cut.pointer(at), "{at} is Cut's, untouched by an edit of Film's own part");
        }
    }

    /// Film EDITS Cut's parts and writes them back in Cut's shape: a text's content and a trim of Cut's element, a renamed bookmark, the
    /// scene's track hidden. Only those fields change; every other field of Cut's comes back exactly.
    #[test]
    fn film_edits_cut_s_parts_and_saves_them_in_cut_s_shape() {
        let cut = with_cut_parts(film_form());
        let mut s = session();
        open_host_file(&mut s, "trip.otio", &serde_json::to_vec(&cut).unwrap()).unwrap();
        edit_clip(&mut s, "T", "Title", |c| {
            let l = c.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
            l.params.insert("text".into(), filmcraft_project::Param::new(ParamValue::Text("Bye".into())));
        });
        // A frame at 30 fps is 4000 of Cut's ticks: shorten "End" by one.
        edit_clip(&mut s, "T", "End", |c| c.duration -= filmcraft_time::FrameRate::FPS_30.frame_duration());
        {
            let p = std::sync::Arc::make_mut(&mut s.project);
            let id = p.items.values().find(|i| i.name == "T").unwrap().id;
            let q = p.sequence_mut(id).unwrap();
            q.markers.iter_mut().find(|m| m.name == "hook").unwrap().name = "hook 2".into();
            let teaser = p.items.values().find(|i| i.name == "Teaser").unwrap().id;
            p.sequence_mut(teaser).unwrap().video_tracks.iter_mut().find(|t| t.name == "Titles").unwrap().enabled = false;
        }
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        // Film's own tracks (V1, A1), then Cut's Titles and its lanes.
        let titles = "/children/0/tracks/children/2";
        assert_eq!(saved.pointer(&format!("{titles}/children/0/metadata/opencut/element/content")), Some(&serde_json::json!("Bye")));
        assert_eq!(saved.pointer(&format!("{titles}/children/2/source_range/duration/value")), Some(&serde_json::json!(116000)));
        assert_eq!(saved.pointer("/children/0/tracks/markers/0/name"), Some(&serde_json::json!("hook 2")));
        assert_eq!(saved.pointer("/children/0/tracks/markers/0/metadata/opencut/bookmark/note"), Some(&serde_json::json!("hook 2")));
        assert_eq!(saved.pointer("/children/1/tracks/children/0/metadata/opencut/track/hidden"), Some(&serde_json::json!(true)));
        // Everything else is exactly the file's.
        let mut want = cut.clone();
        *want.pointer_mut(&format!("{titles}/children/0/metadata/opencut/element/content")).unwrap() = serde_json::json!("Bye");
        *want.pointer_mut(&format!("{titles}/children/2/source_range/duration/value")).unwrap() = serde_json::json!(116000);
        *want.pointer_mut("/children/0/tracks/markers/0/name").unwrap() = serde_json::json!("hook 2");
        *want.pointer_mut("/children/0/tracks/markers/0/metadata/opencut/bookmark/note").unwrap() = serde_json::json!("hook 2");
        *want.pointer_mut("/children/1/tracks/children/0/metadata/opencut/track/hidden").unwrap() = serde_json::json!(true);
        assert_eq!(first_difference(&want, &saved, ""), None);
        // Saved again unedited: the same file.
        let again: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&saved, &again, ""), None);
    }

    /// files/989, Film -> Cut -> Film on a file Film wrote: `film-written.otio` (pinned to Film's writer by the interchange test
    /// `oneos_film_writes_the_file_cut_reads`) as Cut edited it (the OpenCut fork's `oneos-tests/film-written.test.ts` pins Cut's writer to
    /// `cut-edited.otio`: a trim of a clip on Film's track, a title track, a new scene). Film opens Cut's edit with every part as its own
    /// type (Cut's trim reconciled into Film's clip), and an unedited save is the file value for value.
    #[test]
    fn a_file_cut_edited_opens_in_film_and_saves_back_value_for_value() {
        let film: Value = serde_json::from_slice(include_bytes!("../../interchange/tests/it/oneos/film-written.otio")).unwrap();
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-edited.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "Trip.otio", bytes).unwrap();
        assert_eq!(s.host_shared.locked(), Vec::<String>::new(), "nothing of Cut's is locked");
        assert!(s.project.items.values().any(|i| i.name == "Teaser" && i.as_sequence().is_some()), "Cut's scene is a sequence");
        let edit = s.project.items.values().find(|i| i.name == "Edit").unwrap().as_sequence().unwrap();
        assert!(edit.video_tracks.iter().any(|t| t.name == "Titles"), "Cut's title track is a track");
        // Cut's trim of Film's clip (source 24 frames in, 36 long, at 24 fps) is Film's clip now.
        let c = &edit.video_tracks[0].items[0];
        let f = filmcraft_time::FrameRate::FPS_24;
        assert_eq!((c.source_in, c.duration), (f.tick_of(24), f.tick_of(36)));
        assert_eq!(c.label, filmcraft_project::Label::Iris, "it keeps everything else of Film's");
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "Cut's edit comes back from Film value for value");
        assert_eq!(first_difference(&saved["metadata"]["filmcraft"], &film["metadata"]["filmcraft"], ""), None, "Film's own project is Film's");
    }

    /// Film edits the clip Cut trimmed: it is written at Film's rate with Film's exact mirrors, and Cut's own data on it rides along.
    #[test]
    fn film_trims_the_clip_cut_trimmed_and_cut_s_data_on_it_is_kept() {
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-edited.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "Trip.otio", bytes).unwrap();
        let f = filmcraft_time::FrameRate::FPS_24;
        {
            let p = std::sync::Arc::make_mut(&mut s.project);
            let id = p.items.values().find(|i| i.name == "Edit").unwrap().id;
            p.sequence_mut(id).unwrap().video_tracks[0].items[0].duration = f.tick_of(30);
        }
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        let clip = "/children/0/tracks/children/0/children/0";
        assert_eq!(saved.pointer(&format!("{clip}/source_range/duration/value")).and_then(Value::as_f64), Some(30.0));
        assert_eq!(saved.pointer(&format!("{clip}/metadata/filmcraft/duration_ticks")).and_then(Value::as_i64), Some(f.tick_of(30).0));
        assert_eq!(saved.pointer(&format!("{clip}/metadata/opencut")), cut.pointer(&format!("{clip}/metadata/opencut")), "Cut's data on Film's clip");
        assert_eq!(saved.pointer("/children/2"), cut.pointer("/children/2"), "Cut's scene, untouched");
    }

    /// files/989 (Mona 2026-11-09: "yes, so this becomes a tool choice not a file type choicer"): a PURE Cut project (Cut's writer's maximal
    /// project, the OpenCut fork's `oneos-tests/fixtures/cut-project.otio`) opens in Film, saves unedited value for value, and Film's edit of
    /// Cut's bookmark is pinned as `film-saved-cut.otio` (`FILMCRAFT_BLESS=1` writes it), the file the OpenCut fork's
    /// `oneos-tests/foreign-keys.test.ts` opens in Cut's PROJECT mode.
    #[test]
    fn a_pure_cut_project_film_saved_opens_in_cut_s_project_mode() {
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        eprintln!("locked in Film: {:?}", s.host_shared.locked());
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "unedited: Cut's project comes back value for value");
        {
            let p = std::sync::Arc::make_mut(&mut s.project);
            let id = p.items.values().find(|i| i.name == "Main" && i.as_sequence().is_some()).unwrap().id;
            p.sequence_mut(id).unwrap().markers.iter_mut().find(|m| m.name == "Cut here").unwrap().name = "Edited in Film".into();
        }
        let out = save_host_file(&mut s).unwrap_or_else(|e| panic!("Film refused the save: {e}"));
        let edited: Value = serde_json::from_slice(&out).unwrap();
        let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../interchange/tests/it/oneos/film-saved-cut.otio");
        if std::env::var_os("FILMCRAFT_BLESS").is_some() {
            std::fs::write(&golden, &out).unwrap();
        }
        let pinned: Value = serde_json::from_slice(&std::fs::read(&golden).expect("film-saved-cut.otio (FILMCRAFT_BLESS=1 writes it)")).unwrap();
        assert_eq!(first_difference(&pinned, &edited, ""), None, "Film saves Cut's project as it is pinned");
        let mut want = cut.clone();
        *want.pointer_mut("/children/0/tracks/markers/0/name").unwrap() = serde_json::json!("Edited in Film");
        *want.pointer_mut("/children/0/tracks/markers/0/metadata/opencut/bookmark/note").unwrap() = serde_json::json!("Edited in Film");
        assert_eq!(first_difference(&want, &edited, ""), None, "only Film's edit differs");
    }

    /// files/989 (Mona: "yes, so this becomes a tool choice not a file type choicer"): Cut's volume IS Film's clip gain (both decibels).
    /// On Cut's editor-built project, Film's gain change is written as Cut's element volume and nothing else changes; a gain outside
    /// Cut's volume range (-60 to 20 dB) is refused by name at save, never clamped.
    #[test]
    fn film_edits_cut_s_volume_as_its_own_clip_gain() {
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        let (seq, kind, ti, ci) = find(&s, "Main", "Song");
        assert_eq!(s.project.sequence(seq).unwrap().tracks(kind)[ti].items[ci].gain_db, 0.4, "Cut's volume is Film's gain");
        edit_clip(&mut s, "Main", "Song", |c| c.gain_db = -12.5);
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        let mut want = cut.clone();
        *want.pointer_mut("/children/0/tracks/children/4/children/1/metadata/opencut/element/volume").unwrap() = serde_json::json!(-12.5);
        assert_eq!(first_difference(&want, &saved, ""), None, "only the volume changed, as Cut's element volume");
        edit_clip(&mut s, "Main", "Song", |c| c.gain_db = 30.0);
        let e = save_host_file(&mut s).unwrap_err();
        assert!(e.contains("volume-not-in-cut"), "{e}");
    }

    fn keys_of(s: &Session, seq: &str, clip: &str, effect: &str, param: &str) -> Vec<filmcraft_project::Keyframe> {
        let (id, kind, ti, ci) = find(s, seq, clip);
        let c = &s.project.sequence(id).unwrap().tracks(kind)[ti].items[ci];
        c.effects.iter().filter(|e| e.effect == effect).find_map(|e| e.params.get(param)).unwrap_or_else(|| panic!("{clip} has no {effect} {param}")).keyframes.clone()
    }

    /// files/989 step 2 (Mona: "yes, so this becomes a tool choice not a file type choicer"): Film EDITS Cut's keyframes. On Cut's
    /// editor-built project, Cut's channels open as Film's keyframes (Beach's opacity, its position, its blur's intensity), Film's change of a
    /// keyframe and of a value Cut animates is written back as Cut's channel and field, and nothing else changes. Pinned as
    /// `film-keyed-cut.otio` (`FILMCRAFT_BLESS=1` writes it), the file the OpenCut fork's `film-keys.test.ts` opens in Cut's project mode.
    #[test]
    fn film_edits_cut_s_keyframes_and_writes_them_as_cut_s_channels() {
        use filmcraft_project::Interpolation;
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        let locked = s.host_shared.locked();
        assert!(locked.iter().all(|l| l.contains("sticker")), "{locked:?}");
        let op = keys_of(&s, "Main", "Beach", "opacity", "opacity");
        assert_eq!(op.iter().map(|k| (k.time.0, k.value.clone(), k.interp)).collect::<Vec<_>>(), vec![
            (0, ParamValue::Float(0.0), Interpolation::Linear),
            (filmcraft_time::TICKS_PER_SECOND, ParamValue::Float(100.0), Interpolation::Linear),
        ], "Cut's opacity channel is Film's Opacity keyframes");
        let pos = keys_of(&s, "Main", "Beach", "motion", "position");
        // Cut's canvas is 1080 x 1920; its position is from the centre; Y, not animated, is held at its value.
        assert_eq!(pos.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), vec![ParamValue::Vec2(filmcraft_geom::Vec2::new(540.0 + 40.0, 960.0 - 4.0))]);
        assert_eq!(keys_of(&s, "Main", "Beach", "gaussian_blur", "blurriness").len(), 1, "the blur's intensity channel");
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "unedited: Cut's project comes back value for value");
        edit_clip(&mut s, "Main", "Beach", |c| {
            let o = c.effects.iter_mut().find(|e| e.effect == "opacity").unwrap();
            o.params.get_mut("opacity").unwrap().keyframes[1].value = ParamValue::Float(50.0);
            let m = c.effects.iter_mut().find(|e| e.effect == "motion").unwrap();
            m.params.get_mut("rotation").unwrap().value = ParamValue::Float(30.0);
        });
        let out = save_host_file(&mut s).unwrap_or_else(|e| panic!("Film refused the save: {e}"));
        let edited: Value = serde_json::from_slice(&out).unwrap();
        let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../interchange/tests/it/oneos/film-keyed-cut.otio");
        if std::env::var_os("FILMCRAFT_BLESS").is_some() {
            std::fs::write(&golden, &out).unwrap();
        }
        let pinned: Value = serde_json::from_slice(&std::fs::read(&golden).expect("film-keyed-cut.otio (FILMCRAFT_BLESS=1 writes it)")).unwrap();
        assert_eq!(first_difference(&pinned, &edited, ""), None, "Film saves Cut's keyframes as pinned");
        let mut want = cut.clone();
        let el = "/children/0/tracks/children/0/children/0/metadata/opencut/element";
        *want.pointer_mut(&format!("{el}/animations/channels/opacity:value/keys/1/value")).unwrap() = serde_json::json!(0.5);
        *want.pointer_mut(&format!("{el}/transform/rotate")).unwrap() = serde_json::json!(30);
        assert_eq!(first_difference(&want, &edited, ""), None, "only Film's edits differ, as Cut's channel and field");
    }

    /// files/989 step 2: Cut's edit of FILM's keyframes (the OpenCut fork's `cut-keyed.otio`: a position key moved, the rotation, an Opacity
    /// key, a Gaussian Blur key added, a gain changed) opens in Film as Film's own clip, every keyframe Film's, and saves back value for value.
    #[test]
    fn a_cut_edit_of_film_s_keyframes_opens_in_film_as_film_s_own() {
        use filmcraft_project::Interpolation;
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-keyed.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "Keys.otio", bytes).unwrap_or_else(|e| panic!("{e}"));
        let pos = keys_of(&s, "Edit", "Keyed", "motion", "position");
        assert_eq!((pos[1].value.clone(), pos[1].interp, pos[1].out_influence), (ParamValue::Vec2(filmcraft_geom::Vec2::new(1020.0, 500.0)), Interpolation::EaseOut, 0.5));
        let blur = keys_of(&s, "Edit", "Keyed", "gaussian_blur", "blurriness");
        assert_eq!(blur.iter().map(|k| k.interp).collect::<Vec<_>>(), vec![Interpolation::Linear, Interpolation::Hold, Interpolation::Linear]);
        let (id, kind, ti, ci) = find(&s, "Edit", "Keyed sound");
        assert_eq!(s.project.sequence(id).unwrap().tracks(kind)[ti].items[ci].gain_db, -1.0, "Cut's volume is Film's gain");
        let lvl = keys_of(&s, "Edit", "Keyed sound", "volume", "level");
        assert_eq!(lvl.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), vec![ParamValue::Float(-2.0), ParamValue::Float(-8.0)]);
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "unedited, Cut's edit comes back value for value");
    }

    /// Mona 2026-11-09 (files/989): "Build text animation in Cut" and "Add slopes to Film". On Cut's editor-built project, the title's
    /// animated typography opens as Film's own keyframes on its text layer: size (Cut's pixels at Film's scale, its first segment leaving on
    /// a sloped handle) and tracking eased; content, font, alignment, weight, style and decoration held. Cut shows a held channel's own value
    /// before its first key, Film its first key's, so Film keys the clip's start with Cut's own value. Unedited, Cut's project comes back
    /// value for value; Film's edits are written back as Cut's channels, and nothing else changes.
    #[test]
    fn film_edits_cut_s_text_animation_as_its_own_keyframes() {
        use filmcraft_project::Interpolation;
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        let k = 1920.0 / 90.0;
        let size = keys_of(&s, "Main", "Title", "graphic_text", "size");
        assert_eq!(size.len(), 2);
        assert!((size[0].value.as_f64().unwrap() - 64.0 * k).abs() < 1e-9 && (size[1].value.as_f64().unwrap() - 96.0 * k).abs() < 1e-9, "{size:?}");
        assert_eq!((size[0].out_influence, size[0].out_rise.as_ref().map(|r| (r[0] - 12.0 * k).abs() < 1e-6)), (0.25, Some(true)), "Cut's sloped handle is Film's: a quarter of the segment, rising 12 of Cut's pixels");
        let text = keys_of(&s, "Main", "Title", "graphic_text", "text");
        assert_eq!(text.iter().map(|k| (k.time.0, k.value.clone())).collect::<Vec<_>>(), vec![(0, ParamValue::Text("Hello 日本語".into())), (filmcraft_time::TICKS_PER_SECOND, ParamValue::Text("Goodbye".into()))]);
        let font = keys_of(&s, "Main", "Title", "graphic_text", "font");
        assert_eq!(font.iter().map(|k| (k.time.0, k.value.clone(), k.interp)).collect::<Vec<_>>(), vec![
            (0, ParamValue::Text("Inter".into()), Interpolation::Hold),
            (filmcraft_time::TICKS_PER_SECOND, ParamValue::Text("Roboto".into()), Interpolation::Hold),
        ], "keyed at the start with Cut's own font, which Cut shows before its first key");
        let under = keys_of(&s, "Main", "Title", "graphic_text", "underline");
        let struck = keys_of(&s, "Main", "Title", "graphic_text", "strikethrough");
        assert_eq!((under.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), struck.iter().map(|k| k.value.clone()).collect::<Vec<_>>()), (vec![ParamValue::Bool(true), ParamValue::Bool(false)], vec![ParamValue::Bool(false), ParamValue::Bool(true)]));
        assert_eq!(keys_of(&s, "Main", "Title", "graphic_text", "align").last().map(|k| k.value.clone()), Some(ParamValue::Choice(0)));
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "unedited: Cut's project comes back value for value");
        edit_clip(&mut s, "Main", "Title", |c| {
            let l = c.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
            l.params.get_mut("text").unwrap().keyframes[1].value = ParamValue::Text("Farewell".into());
            l.params.get_mut("size").unwrap().keyframes[1].value = ParamValue::Float(80.0 * k);
            l.params.get_mut("tracking").unwrap().put_keyframe(filmcraft_time::Tick(0), ParamValue::Float(2.0));
            l.params.get_mut("faux_bold").unwrap().put_keyframe(filmcraft_time::Tick(filmcraft_time::TICKS_PER_SECOND / 2), ParamValue::Bool(true));
        });
        let edited: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("Film refused the save: {e}"))).unwrap();
        let el = "/children/0/tracks/children/1/children/1/metadata/opencut/element";
        let mut want = cut.clone();
        let ch = |p: &str| format!("{el}/animations/channels/{p}");
        *want.pointer_mut(&format!("{}/keys/1/value", ch("content:value"))).unwrap() = serde_json::json!("Farewell");
        *want.pointer_mut(&format!("{}/keys/1/value", ch("fontSize:value"))).unwrap() = serde_json::json!(80);
        // Film's new tracking key: an id of Film's (the clip and the path), at the start, Cut's linear segment.
        let new_id = edited.pointer(&format!("{}/keys/0/id", ch("letterSpacing:value"))).and_then(Value::as_str).unwrap_or_default().to_string();
        assert!(new_id.starts_with("film-") && new_id.ends_with("-letterSpacing-0-0"), "{new_id}");
        want.pointer_mut(&ch("letterSpacing:value")).unwrap()["keys"].as_array_mut().unwrap().insert(0, serde_json::json!({"id": new_id, "time": 0, "value": 2, "segmentToNext": "linear", "tangentMode": "flat"}));
        // Film's new weight key at half a second; the key Film set at the start for Cut's own weight (bold) is not written as Cut's.
        let normal = want.pointer(&ch("fontWeight:value")).unwrap()["keys"][0].clone();
        want.pointer_mut(&ch("fontWeight:value")).unwrap()["keys"] = serde_json::json!([{"id": "film-fontWeight-0", "time": 60000, "value": "bold"}, normal]);
        assert_eq!(first_difference(&want, &edited, ""), None, "only Film's edits differ, as Cut's channels");
    }

    /// Mona 2026-11-09 (files/989): "Add sRGB channel to Cut". Film eases a colour between colours in sRGB; its edit of Cut's title colour
    /// is written as Cut's sRGB channel (`colorSpace: "srgb"`, Film's sRGB components exactly), which Cut draws as Film does, and it reopens
    /// in Film as the very keyframes. Nothing is refused or locked for it.
    #[test]
    fn film_eases_cut_s_colour_and_writes_it_as_cut_s_srgb_channel() {
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        edit_clip(&mut s, "Main", "Title", |c| {
            let l = c.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
            l.params.get_mut("fill_color").unwrap().put_keyframe(filmcraft_time::Tick(0), ParamValue::Color([0.0, 0.5, 1.0, 1.0]));
        });
        let out = save_host_file(&mut s).unwrap_or_else(|e| panic!("Film refused the save: {e}"));
        let edited: Value = serde_json::from_slice(&out).unwrap();
        let el = "/children/0/tracks/children/1/children/1/metadata/opencut/element/animations";
        assert_eq!(edited.pointer(&format!("{el}/bindings/color/colorSpace")), Some(&serde_json::json!("srgb")));
        let g = edited.pointer(&format!("{el}/channels/color:g/keys")).unwrap().as_array().unwrap();
        assert_eq!(g.iter().map(|k| k["value"].as_f64().unwrap()).collect::<Vec<_>>(), vec![0.5, 0.0], "sRGB components, as Film keys them");
        let fill = keys_of(&s, "Main", "Title", "graphic_text", "fill_color");
        let mut again = session();
        open_host_file(&mut again, "trip.otio", &out).unwrap();
        assert_eq!(keys_of(&again, "Main", "Title", "graphic_text", "fill_color"), fill, "reopened in Film as the very keyframes");
        let saved: Value = serde_json::from_slice(&save_host_file(&mut again).unwrap()).unwrap();
        assert_eq!(first_difference(&edited, &saved, ""), None);
    }

    /// files/989 step 2: what Cut does not animate (a text background's opacity) is refused by name when Film keyframes it, never dropped.
    #[test]
    fn a_keyframe_cut_has_no_channel_for_is_refused_by_name() {
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        edit_clip(&mut s, "Main", "Title", |c| {
            let l = c.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
            l.params.get_mut("background_opacity").unwrap().put_keyframe(filmcraft_time::Tick(0), ParamValue::Float(40.0));
        });
        let e = save_host_file(&mut s).unwrap_err();
        assert!(e.contains("keyframes-not-in-cut") && e.contains("background_opacity") && e.contains("Title"), "{e}");
    }
}
