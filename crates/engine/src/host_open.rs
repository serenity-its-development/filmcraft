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
            Self::MediaNotReferenced { name, media } => {
                write!(f, "media-not-referenced: {name} names the medium {media}, which is not a OneOS file and version")
            }
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
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                x.iter().zip(y).enumerate().find_map(|(i, (p, q))| first_difference(p, q, &format!("{at}/{i}")))
            }
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

    /// files/989 step 2e: Cut's parts of a Film file as CUT writes them, of elements the EDITOR built (the OpenCut fork's
    /// `oneos-tests/editor/film-cut-parts.test.ts` writes `cut-parts-film.otio` into Film's `film-written.otio`): a text track "Titles" (the
    /// maximal project's editor-built title, its typography keyed, a gap, and a text "End"), a graphic track "Shapes" whose two shapes
    /// overlap (Cut's stacked lanes), a bookmark "hook" on Film's "Edit" and a scene of Cut's, "Teaser". Replaces the hand-made
    /// `with_cut_parts` (a text element of four fields, one of them not Cut's).
    fn cut_parts() -> Value {
        serde_json::from_slice(include_bytes!("../../interchange/tests/it/oneos/cut-parts-film.otio")).unwrap()
    }
    const TITLES: &str = "/children/0/tracks/children/7";

    /// files/989 (Mona 2026-11-09: "film should be able to edit cut though"): Cut's parts of a Film file open in Film as Film's own types (a
    /// text track as a track of graphic clips with a text layer, Cut's stacked lanes as tracks, its scene as a sequence, its bookmark as a
    /// marker), nothing locked; an unedited save is the file value for value, and so is a save after Film edits only its own part.
    #[test]
    fn cut_parts_in_a_film_file_open_as_film_types_and_save_back_value_for_value() {
        let cut = cut_parts();
        let bytes = serde_json::to_vec(&cut).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", &bytes).unwrap();
        assert!(!s.is_dirty());
        assert_eq!(s.host_shared.locked(), Vec::<String>::new(), "nothing of Cut's is locked");
        assert_eq!(cut.pointer(&format!("{TITLES}/name")), Some(&serde_json::json!("Titles")));
        let (_, _, _, _) = find(&s, "Teaser", "Title");
        let (seq, kind, ti, ci) = find(&s, "Edit", "End");
        let q = s.project.sequence(seq).unwrap();
        assert_eq!(kind, TrackKind::Video);
        assert_eq!(q.video_tracks[ti].name, "Titles");
        assert_eq!(text_of(&q.video_tracks[ti].items[ci]), "The end");
        assert!(q.video_tracks.iter().any(|t| t.name == "Shapes (lane 2)"), "Cut's lanes are tracks");
        assert!(q.markers.iter().any(|m| m.name == "hook"), "Cut's bookmark is a marker");
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "unedited: the file comes back value for value");
        std::sync::Arc::make_mut(&mut s.project).notes = "Film was here".into();
        let edited: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_ne!(first_difference(&cut, &edited, ""), None, "Film's edit is saved");
        for at in [TITLES, "/children/0/tracks/children/8", "/children/0/tracks/markers", "/children/2", "/metadata/opencut"] {
            assert_eq!(edited.pointer(at), cut.pointer(at), "{at} is Cut's, untouched by an edit of Film's own part");
        }
    }

    /// Film EDITS Cut's parts and writes them back in Cut's shape: a text's content and a trim of Cut's element, a renamed bookmark, the
    /// scene's track hidden. Only those fields change; every other field of Cut's comes back exactly.
    #[test]
    fn film_edits_cut_s_parts_and_saves_them_in_cut_s_shape() {
        let cut = cut_parts();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", &serde_json::to_vec(&cut).unwrap()).unwrap();
        edit_clip(&mut s, "Edit", "End", |c| {
            let l = c.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
            l.params.insert("text".into(), filmcraft_project::Param::new(ParamValue::Text("Bye".into())));
        });
        // A frame at 30 fps is 4000 of Cut's ticks: shorten "End" by one.
        edit_clip(&mut s, "Edit", "End", |c| c.duration -= filmcraft_time::FrameRate::FPS_30.frame_duration());
        {
            let p = std::sync::Arc::make_mut(&mut s.project);
            let id = p.items.values().find(|i| i.name == "Edit").unwrap().id;
            let q = p.sequence_mut(id).unwrap();
            q.markers.iter_mut().find(|m| m.name == "hook").unwrap().name = "hook 2".into();
            let teaser = p.items.values().find(|i| i.name == "Teaser").unwrap().id;
            p.sequence_mut(teaser).unwrap().video_tracks.iter_mut().find(|t| t.name == "Titles").unwrap().enabled = false;
        }
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        let end = format!("{TITLES}/children/2");
        assert_eq!(saved.pointer(&format!("{end}/metadata/opencut/element/content")), Some(&serde_json::json!("Bye")));
        assert_eq!(saved.pointer(&format!("{end}/source_range/duration/value")), Some(&serde_json::json!(116000)));
        assert_eq!(saved.pointer("/children/0/tracks/markers/0/name"), Some(&serde_json::json!("hook 2")));
        assert_eq!(saved.pointer("/children/0/tracks/markers/0/metadata/opencut/bookmark/note"), Some(&serde_json::json!("hook 2")));
        assert_eq!(saved.pointer("/children/2/tracks/children/0/metadata/opencut/track/hidden"), Some(&serde_json::json!(true)));
        // Everything else is exactly the file's.
        let mut want = cut.clone();
        *want.pointer_mut(&format!("{end}/metadata/opencut/element/content")).unwrap() = serde_json::json!("Bye");
        *want.pointer_mut(&format!("{end}/source_range/duration/value")).unwrap() = serde_json::json!(116000);
        *want.pointer_mut("/children/0/tracks/markers/0/name").unwrap() = serde_json::json!("hook 2");
        *want.pointer_mut("/children/0/tracks/markers/0/metadata/opencut/bookmark/note").unwrap() = serde_json::json!("hook 2");
        *want.pointer_mut("/children/2/tracks/children/0/metadata/opencut/track/hidden").unwrap() = serde_json::json!(true);
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

    /// files/989 (measured 2026-11-10): Cut split Film's linked picture and sound (the OpenCut fork's `oneos-tests/linked.test.ts` writes
    /// `cut-split-film.otio`). Before, the right halves were Cut's own elements and Film read them as bare clips; now they are Film's clips in
    /// Film's shape: each right half has its left half's look, gain and label, the right halves are linked under their own number, and an
    /// unedited save is the file value for value.
    #[test]
    fn a_film_clip_cut_split_keeps_film_s_look_and_link() {
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-split-film.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "Trip.otio", bytes).unwrap();
        assert_eq!(s.host_shared.locked(), Vec::<String>::new(), "nothing is locked");
        let edit = s.project.items.values().find(|i| i.name == "Edit").unwrap().as_sequence().unwrap();
        for (kind, t) in [(TrackKind::Video, &edit.video_tracks[0]), (TrackKind::Audio, &edit.audio_tracks[0])] {
            let (l, r) = (&t.items[0], &t.items[1]);
            assert_eq!(l.end(), r.start, "{kind:?}: the halves meet");
            assert_eq!((l.link, r.link), (Some(13), Some(14)), "{kind:?}: the left halves keep Film's link, the right halves have their own");
            assert_eq!((r.label, r.custom_label_color, r.gain_db, r.scale_to_frame), (l.label, l.custom_label_color, l.gain_db, l.scale_to_frame), "{kind:?}");
            let static_of = |c: &filmcraft_project::TrackItem| serde_json::to_value(&c.effects).unwrap();
            assert_eq!(first_difference(&static_of(l), &static_of(r), ""), None, "{kind:?}: the right half has Film's look");
        }
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "Cut's split comes back from Film value for value");
    }

    /// files/989 (2026-11-10, the gap "a copy of Film's clip is Film's clip" left): Film's keyed clips (`film-keyed.otio`) split by the
    /// editor's own split in Cut 0.375 s in, and the picture duplicated and pasted onto Film's second track, as the OpenCut fork's
    /// `oneos-tests/editor/film-copy-keys.test.ts` writes them (`cut-split-keyed.otio`). In Film each copy is Film's clip, nothing locked,
    /// and every keyed parameter draws at each moment of the medium as the clip it came from did (to the editor's whole-tick handles: a
    /// few millionths of the parameter's range); an unedited save is the file.
    #[test]
    fn a_film_clip_cut_split_keeps_its_keyframes_at_their_moment() {
        use filmcraft_project::ParamValue;
        let before = include_bytes!("../../interchange/tests/it/oneos/film-keyed.otio");
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-split-keyed.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut was = session();
        open_host_file(&mut was, "Keys.otio", before).unwrap();
        let mut s = session();
        open_host_file(&mut s, "Keys.otio", bytes).unwrap();
        assert_eq!(s.host_shared.locked(), Vec::<String>::new(), "nothing is locked");
        let clips = |s: &Session, name: &str| -> Vec<filmcraft_project::TrackItem> {
            let seq = s.project.items.values().find(|i| i.name == "Edit").unwrap().as_sequence().unwrap();
            let mut v: Vec<_> = seq.video_tracks.iter().chain(&seq.audio_tracks).flat_map(|t| t.items.iter()).filter(|c| c.name == name).cloned().collect();
            v.sort_by_key(|c| (c.start, c.source_in));
            v
        };
        let comps = |v: &ParamValue| -> Vec<f64> { v.as_f64().map(|x| vec![x]).or_else(|| v.as_vec2().map(|p| vec![p.x, p.y])).expect("a number or a point") };
        let mut drawn = 0;
        for (name, copies) in [("Keyed", 4), ("Keyed sound", 2)] {
            let from = clips(&was, name);
            assert_eq!(from.len(), 1);
            let from = &from[0];
            let to = clips(&s, name);
            assert_eq!(to.len(), copies, "{name}: the halves (and the picture's duplicate and paste)");
            for c in &to {
                for fx in from.effects.iter() {
                    for (pname, p) in fx.params.iter().filter(|(_, p)| p.is_animated()) {
                        let q = c
                            .effects
                            .iter()
                            .filter(|e| e.effect == fx.effect)
                            .find_map(|e| e.params.get(pname))
                            .unwrap_or_else(|| panic!("{name}: {} {pname} is kept", fx.effect));
                        assert!(q.is_animated(), "{name} at {:?}: {} {pname} stays keyed", c.start, fx.effect);
                        let vals: Vec<f64> = p.keyframes.iter().flat_map(|k| comps(&k.value)).collect();
                        let range = (vals.iter().cloned().fold(f64::MIN, f64::max) - vals.iter().cloned().fold(f64::MAX, f64::min)).max(1.0);
                        for i in 0..=8 {
                            let local = filmcraft_time::Tick(c.duration.0 * i / 8);
                            let medium = c.source_in.0 + local.0;
                            let want = comps(&p.value_at(filmcraft_time::Tick(medium - from.source_in.0)));
                            let got = comps(&q.value_at(local));
                            for (g, w) in got.iter().zip(&want) {
                                assert!(
                                    (g - w).abs() <= 1e-5 * range,
                                    "{name} at {:?}: {} {pname} at medium tick {medium} is {g}, {w} before",
                                    c.start,
                                    fx.effect
                                );
                            }
                            drawn += 1;
                        }
                    }
                }
            }
        }
        assert!(drawn > 0, "the copies were drawn");
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "Cut's split comes back from Film value for value");
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

    /// The clip named `name` on `seq`'s tracks of `kind`.
    fn clip_of(s: &Session, seq: &str, kind: TrackKind, name: &str) -> filmcraft_project::TrackItem {
        let q = s.project.items.values().find(|i| i.name == seq && i.as_sequence().is_some()).and_then(|i| i.as_sequence()).unwrap();
        q.tracks(kind).iter().flat_map(|t| t.items.iter()).find(|c| c.name == name).cloned().unwrap_or_else(|| panic!("no {kind:?} clip {name}"))
    }

    /// Edit every clip named `name` on `seq` (a picture and its sound alike) with the track it is on.
    fn edit_all(s: &mut Session, seq: &str, name: &str, f: impl Fn(TrackKind, &mut filmcraft_project::TrackItem)) {
        let p = std::sync::Arc::make_mut(&mut s.project);
        let id = p.items.values().find(|i| i.name == seq && i.as_sequence().is_some()).unwrap().id;
        let q = p.sequence_mut(id).unwrap();
        for kind in [TrackKind::Video, TrackKind::Audio] {
            for t in q.tracks_mut(kind) {
                for c in t.items.iter_mut().filter(|c| c.name == name) {
                    f(kind, c);
                }
            }
        }
    }

    /// files/989 step 2b (Mona: "Build linked audio, lock multicam"), Film's half: Cut's video element with its sound on (Beach, on Cut's
    /// editor-built project) opens in Film as a picture and a linked audio clip in step with it, the sound holding the element's volume
    /// (Film's gain), mute and volume channel (Film's Volume level over the gain); the picture holds none. An unedited save is the file;
    /// Film's change of the sound is written as the ONE element's volume, mute and channel; a sound moved apart from its picture is refused
    /// by name (`sound-not-in-cut`); the sound Film removes turns the element's sound off; a split in Film (picture and sound cut together)
    /// writes the right halves as one new element with its sound on.
    #[test]
    fn cut_s_video_with_sound_opens_in_film_as_a_picture_and_its_linked_audio() {
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        let (pic, snd) = (clip_of(&s, "Main", TrackKind::Video, "Beach"), clip_of(&s, "Main", TrackKind::Audio, "Beach"));
        assert!(pic.link.is_some() && pic.link == snd.link, "the picture and its sound are linked: {:?} {:?}", pic.link, snd.link);
        assert_eq!((pic.start, pic.source_in, pic.duration), (snd.start, snd.source_in, snd.duration), "in step");
        assert_eq!((pic.gain_db, snd.gain_db, snd.enabled), (0.0, 0.7, true), "Cut's volume and mute are the sound's");
        let level = |c: &filmcraft_project::TrackItem| c.effects.iter().find(|e| e.effect == "volume").unwrap().params["level"].keyframes.clone();
        let keys = level(&snd);
        assert_eq!(keys.len(), 1, "Cut's volume channel is the sound's Volume level");
        assert!((keys[0].value.as_f64().unwrap() - (0.2 - 0.7)).abs() < 1e-9, "{:?}", keys[0].value);
        let beach = clip_ptr(&cut, "Beach", "").unwrap();
        let el = format!("{beach}/metadata/opencut/element");
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "unedited: the file value for value");

        // Film's edit of the sound is the element's volume, mute and channel.
        edit_all(&mut s, "Main", "Beach", |k, c| {
            if k == TrackKind::Audio {
                c.gain_db = -6.0;
                c.enabled = false;
            }
        });
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        assert_eq!(saved.pointer(&format!("{el}/volume")), Some(&serde_json::json!(-6)));
        assert_eq!(saved.pointer(&format!("{el}/muted")), Some(&serde_json::json!(true)));
        let key = format!("{el}/animations/channels/volume:value/keys/0/value");
        let v = saved.pointer(&key).and_then(Value::as_f64).unwrap();
        assert!((v - (0.2 - 0.7 - 6.0)).abs() < 1e-6, "the channel is Film's level over the new gain: {v}");
        let mut want = cut.clone();
        for k in ["volume", "muted"] {
            *want.pointer_mut(&format!("{el}/{k}")).unwrap() = saved.pointer(&format!("{el}/{k}")).unwrap().clone();
        }
        *want.pointer_mut(&key).unwrap() = saved.pointer(&key).unwrap().clone();
        assert_eq!(first_difference(&want, &saved, ""), None, "only the sound changed, as the one element's");

        // A sound moved apart from its picture is refused by name; in step again, it saves.
        edit_all(&mut s, "Main", "Beach", |k, c| {
            if k == TrackKind::Audio {
                c.start += filmcraft_time::Tick(1_000);
            }
        });
        let e = save_host_file(&mut s).unwrap_err();
        assert!(e.contains("sound-not-in-cut"), "{e}");
        edit_all(&mut s, "Main", "Beach", |k, c| {
            if k == TrackKind::Audio {
                c.start -= filmcraft_time::Tick(1_000);
            }
        });
        save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"));

        // Film splits Beach (its picture and sound together): the right halves are ONE new element of Cut's, its sound on.
        {
            let p = std::sync::Arc::make_mut(&mut s.project);
            let id = p.items.values().find(|i| i.name == "Main" && i.as_sequence().is_some()).unwrap().id;
            let link = p.alloc_id();
            let mut rights = Vec::new();
            for kind in [TrackKind::Video, TrackKind::Audio] {
                let q = p.sequence(id).unwrap();
                let (ti, ci) = q.tracks(kind).iter().enumerate().find_map(|(t, tr)| tr.items.iter().position(|c| c.name == "Beach").map(|c| (t, c))).unwrap();
                let mut right = q.tracks(kind)[ti].items[ci].clone();
                let half = filmcraft_time::Tick(right.duration.0 / 2);
                right.id = filmcraft_project::ClipId(p.alloc_id());
                right.start += half;
                right.source_in += half;
                right.duration -= half;
                right.link = Some(link);
                rights.push((kind, ti, ci, half, right));
            }
            let q = p.sequence_mut(id).unwrap();
            for (kind, ti, ci, half, right) in rights {
                let t = &mut q.tracks_mut(kind)[ti];
                t.items[ci].duration = half;
                t.items.push(right);
                t.sort();
            }
        }
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        let main = saved.pointer("/children/0/tracks/children/0/children").and_then(Value::as_array).unwrap();
        let halves: Vec<&Value> = main.iter().filter(|c| c.get("name").and_then(Value::as_str) == Some("Beach")).collect();
        assert_eq!(halves.len(), 2, "the halves are two elements: {main:?}");
        let right = halves[1].pointer("/metadata/opencut/element").unwrap();
        assert_eq!(right.get("isSourceAudioEnabled"), Some(&serde_json::json!(true)), "the right half plays its sound");
        assert_eq!((right.get("volume"), right.get("muted")), (Some(&serde_json::json!(-6)), Some(&serde_json::json!(true))), "its sound's");
        assert_eq!(right.get("link"), None, "a picture and its own sound are one element in Cut, no link label");

        // The sound Film removes: Cut's element plays none.
        {
            let p = std::sync::Arc::make_mut(&mut s.project);
            let id = p.items.values().find(|i| i.name == "Main" && i.as_sequence().is_some()).unwrap().id;
            for t in p.sequence_mut(id).unwrap().audio_tracks.iter_mut() {
                t.items.retain(|c| c.name != "Beach");
            }
        }
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        let main = saved.pointer("/children/0/tracks/children/0/children").and_then(Value::as_array).unwrap();
        for c in main.iter().filter(|c| c.get("name").and_then(Value::as_str) == Some("Beach")) {
            assert_eq!(c.pointer("/metadata/opencut/element/isSourceAudioEnabled"), Some(&serde_json::json!(false)), "{c}");
        }
    }

    /// The JSON pointer of the first clip named `name` in `v`.
    fn clip_ptr(v: &Value, name: &str, at: &str) -> Option<String> {
        match v {
            Value::Object(o) if o.get("OTIO_SCHEMA").and_then(Value::as_str) == Some("Clip.2") && o.get("name").and_then(Value::as_str) == Some(name) => {
                Some(at.to_string())
            }
            Value::Object(o) => o.iter().find_map(|(k, x)| clip_ptr(x, name, &format!("{at}/{k}"))),
            Value::Array(a) => a.iter().enumerate().find_map(|(i, x)| clip_ptr(x, name, &format!("{at}/{i}"))),
            _ => None,
        }
    }

    /// files/989 part 2 ("Masks, both ways"): Film EDITS Cut's masks as its own. On Cut's editor-built project (Beach's text mask taken
    /// out for the pinned file Cut opens), Beach's masks open as Film's masks on the clip's Opacity, one of each of Cut's
    /// types, each the path Cut's editor draws (an open custom path kept with its mode None, as Cut does not draw it), and an unedited
    /// save is the file. Film's change of a feather and the masks Film adds (on Beach, and on the Logo, which had none) are written back in
    /// Cut's shape (a feather Film set in the mask's own units, Film's mask as Cut's custom path) and nothing else changes. With its text mask
    /// (Film's text mask, files/989 tool choice), Beach's masks open as Film's too, nothing carried: an unedited save is the file, and Film's
    /// change of the text and a mask Film adds beside it are written in Cut's shape.
    #[test]
    fn film_edits_cut_s_masks_as_its_own_and_writes_them_in_cut_s_shape() {
        use filmcraft_project::{Mask, MaskMode, MaskPath};
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let mut cut: Value = serde_json::from_slice(bytes).unwrap();
        let beach = format!("{}/metadata/opencut/element/masks", clip_ptr(&cut, "Beach", "").unwrap());
        let logo = format!("{}/metadata/opencut/element/masks", clip_ptr(&cut, "Logo", "").unwrap());
        cut.pointer_mut(&beach).unwrap().as_array_mut().unwrap().retain(|m| m["type"] != "text");
        let file = serde_json::to_vec(&cut).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", &file).unwrap();
        let masks_of = |s: &Session, clip: &str| {
            let (seq, kind, ti, ci) = find(s, "Main", clip);
            s.project.sequence(seq).unwrap().tracks(kind)[ti].items[ci].effects.iter().find(|e| e.effect == "opacity").unwrap().masks.clone()
        };
        let masks = masks_of(&s, "Beach");
        assert_eq!(
            masks.iter().map(|m| (m.name.as_str(), m.mode)).collect::<Vec<_>>(),
            vec![
                ("Split", MaskMode::Add),
                ("Cinematic Bars", MaskMode::Add),
                ("Rectangle", MaskMode::Add),
                ("Ellipse", MaskMode::Add),
                ("Heart", MaskMode::Add),
                ("Diamond", MaskMode::Add),
                ("Star", MaskMode::Add),
                ("Custom", MaskMode::None)
            ],
            "each of Cut's masks is Film's"
        );
        let unedited: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        assert_eq!(first_difference(&cut, &unedited, ""), None, "an unedited save is the file");
        let hole = || {
            let mut m = Mask::new("Hole", MaskPath::ellipse(filmcraft_geom::Vec2::new(960.0, 540.0), filmcraft_geom::Vec2::new(40.0, 30.0)));
            m.mode = MaskMode::Subtract;
            m
        };
        edit_clip(&mut s, "Main", "Beach", |c| {
            let o = c.effects.iter_mut().find(|e| e.effect == "opacity").unwrap();
            o.masks[2].feather = filmcraft_project::Param::new(ParamValue::Float(6.0));
            o.masks.push(hole());
        });
        edit_clip(&mut s, "Main", "Logo", |c| c.effects.iter_mut().find(|e| e.effect == "opacity").unwrap().masks.push(hole()));
        let out = save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"));
        // Pinned as `film-masked-cut.otio` (`FILMCRAFT_BLESS=1` writes it), the file the OpenCut fork's `film-masks.test.ts` opens in Cut.
        let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../interchange/tests/it/oneos/film-masked-cut.otio");
        if std::env::var_os("FILMCRAFT_BLESS").is_some() {
            std::fs::write(&golden, &out).unwrap();
        }
        assert_eq!(std::fs::read(&golden).expect("film-masked-cut.otio (FILMCRAFT_BLESS=1 writes it)"), out, "Film saves Cut's masks as pinned");
        let saved: Value = serde_json::from_slice(&out).unwrap();
        let got = saved.pointer(&beach).unwrap().as_array().unwrap().clone();
        let logo_got = saved.pointer(&logo).unwrap_or_else(|| panic!("the Logo has masks")).clone();
        let mut want = cut.clone();
        let m2 = want.pointer_mut(&format!("{beach}/2/params")).unwrap();
        m2["feather"] = serde_json::json!(6);
        m2["expansion"] = serde_json::json!(0);
        want.pointer_mut(&beach).unwrap().as_array_mut().unwrap().push(got[8].clone());
        let logo_el = logo.trim_end_matches("/masks").to_string();
        want.pointer_mut(&logo_el).unwrap()["masks"] = logo_got.clone();
        assert_eq!(first_difference(&want, &saved, ""), None, "only the rectangle's feather changed and Film's masks were added");
        for m in [&got[8], &logo_got[0]] {
            assert_eq!(
                (&m["type"], &m["name"], &m["params"]["mode"], &m["params"]["units"], &m["params"]["closed"]),
                (
                    &serde_json::json!("custom"),
                    &serde_json::json!("Hole"),
                    &serde_json::json!("subtract"),
                    &serde_json::json!("clip"),
                    &serde_json::json!(true)
                )
            );
        }
        // With its text mask, Beach's masks open as Film's, the text mask as Film's text mask.
        let whole: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        let masks = masks_of(&s, "Beach");
        assert_eq!(masks.len(), 9, "nothing carried");
        let text = masks[7].text.as_ref().expect("Beach's text mask is Film's text mask");
        assert_eq!((masks[7].name.as_str(), text.content.as_str(), text.size), ("Text", "Mask", 15.0));
        let unedited: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        assert_eq!(first_difference(&whole, &unedited, ""), None, "an unedited save is the file");
        edit_clip(&mut s, "Main", "Beach", |c| {
            let o = c.effects.iter_mut().find(|e| e.effect == "opacity").unwrap();
            o.masks[7].text.as_mut().unwrap().content = "Film".into();
            o.masks.push(hole());
        });
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        let got = saved.pointer(&beach).unwrap().as_array().unwrap().clone();
        let mut want = whole.clone();
        let list = want.pointer_mut(&beach).unwrap().as_array_mut().unwrap();
        list[7]["params"]["content"] = serde_json::json!("Film");
        list.push(got[9].clone());
        assert_eq!(first_difference(&want, &saved, ""), None, "only the text changed and Film's mask was added");
        assert_eq!((&got[7]["type"], &got[9]["type"], &got[9]["name"]), (&serde_json::json!("text"), &serde_json::json!("custom"), &serde_json::json!("Hole")));
    }

    /// files/989 (Mona 2026-11-10: "Carry it untouched"): a Cut mask whose kind, units, mode, tracking or text weight is not one of Cut's
    /// own values (damaged data) is carried byte for byte by Film and written back unchanged, and the track stays editable around it. On
    /// Cut's editor-built project with one of Beach's masks damaged each way: nothing of Beach's track is locked, Beach opens with none of
    /// its masks in Film (the list is carried whole), an unedited save is the file, Film's rotation of Beach is written and Beach's masks are
    /// the file's very text, and a mask Film adds beside them is refused at save by name (`masks-carried`), nothing written.
    #[test]
    fn a_damaged_cut_mask_is_carried_untouched_and_its_track_stays_editable() {
        use filmcraft_project::{Mask, MaskPath};
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let damages: [(&str, fn(&mut Value)); 5] = [
            ("kind", |m| m["type"] = serde_json::json!("blob")),
            ("units", |m| m["params"]["units"] = serde_json::json!("inches")),
            ("mode", |m| m["params"]["mode"] = serde_json::json!("screen")),
            ("tracking", |m| m["track"] = serde_json::json!("perspective")),
            ("text weight", |m| m["params"]["fontWeight"] = serde_json::json!("600")),
        ];
        for (what, damage) in damages {
            let mut cut: Value = serde_json::from_slice(bytes).unwrap();
            let el = format!("{}/metadata/opencut/element", clip_ptr(&cut, "Beach", "").unwrap());
            let masks = cut.pointer_mut(&format!("{el}/masks")).unwrap().as_array_mut().unwrap();
            let at = if what == "text weight" { masks.iter().position(|m| m["type"] == "text").unwrap() } else { 2 };
            damage(&mut masks[at]);
            let file = serde_json::to_vec(&cut).unwrap();
            let mut s = session();
            open_host_file(&mut s, "trip.otio", &file).unwrap_or_else(|e| panic!("{what}: {e}"));
            let locked = s.host_shared.locked();
            assert!(locked.iter().all(|l| l.contains("sticker")), "{what}: the track stays editable: {locked:?}");
            let (seq, kind, ti, ci) = find(&s, "Main", "Beach");
            let beach = &s.project.sequence(seq).unwrap().tracks(kind)[ti].items[ci];
            assert!(beach.effects.iter().all(|e| e.masks.is_empty()), "{what}: Beach's masks are carried, none of them Film's");
            let unedited: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{what}: {e}"))).unwrap();
            assert_eq!(first_difference(&cut, &unedited, ""), None, "{what}: an unedited save is the file");
            edit_clip(&mut s, "Main", "Beach", |c| {
                c.effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("rotation").unwrap().value = ParamValue::Float(30.0);
            });
            let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{what}: {e}"))).unwrap();
            let mut want = cut.clone();
            want.pointer_mut(&el).unwrap()["transform"]["rotate"] = serde_json::json!(30);
            assert_eq!(first_difference(&want, &saved, ""), None, "{what}: only Film's rotation is written");
            let text = |v: &Value| serde_json::to_string(v.pointer(&format!("{el}/masks")).unwrap()).unwrap();
            assert_eq!(text(&saved), text(&cut), "{what}: Beach's masks are written back byte for byte");
            edit_clip(&mut s, "Main", "Beach", |c| {
                let o = c.effects.iter_mut().find(|e| e.effect == "opacity").unwrap();
                o.masks.push(Mask::new("Hole", MaskPath::ellipse(filmcraft_geom::Vec2::new(960.0, 540.0), filmcraft_geom::Vec2::new(40.0, 30.0))));
            });
            let e = save_host_file(&mut s).unwrap_err();
            assert!(e.contains("masks-carried:") && e.contains("Beach"), "{what}: {e}");
        }
    }

    /// files/989 (Mona 2026-11-09: "Build canvas feather in Film"): on Cut's editor-built project with Beach scaled unevenly (160 % wide,
    /// 80 % tall) and its rectangle feathered and its ellipse grown in the canvas's pixels, Beach's masks open in Film as its own (not
    /// carried), measured in the canvas's pixels with Cut's numbers; an unedited save is the file, and Film's change of the feather is
    /// written as Cut's canvas feather and nothing else.
    #[test]
    fn cut_s_canvas_feather_on_an_uneven_scale_opens_in_film_as_its_own() {
        use filmcraft_project::MaskUnits;
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let mut cut: Value = serde_json::from_slice(bytes).unwrap();
        let el = format!("{}/metadata/opencut/element", clip_ptr(&cut, "Beach", "").unwrap());
        let e = cut.pointer_mut(&el).unwrap();
        e["masks"].as_array_mut().unwrap().retain(|m| m["type"] != "text");
        e["transform"]["scaleX"] = serde_json::json!(1.6);
        e["transform"]["scaleY"] = serde_json::json!(0.8);
        e["masks"][2]["params"]["feather"] = serde_json::json!(12);
        e["masks"][3]["params"]["expansion"] = serde_json::json!(4);
        let file = serde_json::to_vec(&cut).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", &file).unwrap();
        let (seq, kind, ti, ci) = find(&s, "Main", "Beach");
        let clip = &s.project.sequence(seq).unwrap().tracks(kind)[ti].items[ci];
        let masks = &clip.effects.iter().find(|e| e.effect == "opacity").unwrap().masks;
        assert_eq!(masks.len(), 8, "Beach's masks are Film's, not carried");
        let measures = |m: &filmcraft_project::Mask| (m.units, m.feather.value.as_f64(), m.expansion.value.as_f64());
        assert_eq!(measures(&masks[2]), (MaskUnits::Canvas, Some(12.0), Some(0.0)));
        assert_eq!(measures(&masks[3]), (MaskUnits::Canvas, Some(0.0), Some(4.0)));
        let unedited: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        assert_eq!(first_difference(&cut, &unedited, ""), None, "an unedited save is the file");
        edit_clip(&mut s, "Main", "Beach", |c| {
            c.effects.iter_mut().find(|e| e.effect == "opacity").unwrap().masks[2].feather = filmcraft_project::Param::new(ParamValue::Float(20.0));
        });
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        let mut want = cut.clone();
        let p = want.pointer_mut(&format!("{el}/masks/2/params")).unwrap();
        p["feather"] = serde_json::json!(20);
        p["expansion"] = serde_json::json!(0);
        assert_eq!(first_difference(&want, &saved, ""), None, "only the rectangle's feather changed, in the canvas's pixels");
    }

    /// files/989 fix 4a (tool choice: both tools draw the same): Cut fits a picture in its canvas (contain) before its scale, so its media
    /// open in Film fitted to the frame (Scale to Frame Size) with Cut's scale as Film's. On Cut's editor-built project (a 1080 x 1920
    /// canvas) the 1920 x 1080 Beach, scaled 1.25, draws 1350 sequence pixels wide in Film as in Cut, and the 512-pixel Logo fills the
    /// canvas's width. A picture Film does not fit is written with Cut's scale of the same size; keyed, it is refused by name.
    #[test]
    fn cut_s_pictures_open_fitted_in_film_and_draw_at_cut_s_size() {
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        let drawn_width = |s: &Session, clip: &str, px: (u32, u32)| {
            let (seq, kind, ti, ci) = find(s, "Main", clip);
            let q = s.project.sequence(seq).unwrap();
            let c = &q.tracks(kind)[ti].items[ci];
            assert!(c.scale_to_frame, "{clip} is fitted to the frame");
            let m = filmcraft_render::motion_matrix(q, c, px, None, c.source_in);
            let (a, b) = (m.apply(filmcraft_geom::Vec2::new(0.0, 0.0)), m.apply(filmcraft_geom::Vec2::new(px.0 as f64, 0.0)));
            (b - a).length()
        };
        let w = drawn_width(&s, "Beach", (1920, 1080));
        assert!((w - 1080.0 * 1.25).abs() < 1e-6, "Beach draws {w} wide");
        let w = drawn_width(&s, "Logo", (512, 512));
        assert!((w - 1080.0).abs() < 1e-6, "Logo draws {w} wide");
        let unedited: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        assert_eq!(first_difference(&cut, &unedited, ""), None, "an unedited save is the file");
        // Film draws the Logo at its own 512 pixels: Cut's scale of that size is 512 / 1080 of the canvas's width.
        edit_clip(&mut s, "Main", "Logo", |c| c.scale_to_frame = false);
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap_or_else(|e| panic!("{e}"))).unwrap();
        let logo = format!("{}/metadata/opencut/element/transform", clip_ptr(&cut, "Logo", "").unwrap());
        let t = saved.pointer(&logo).unwrap();
        for k in ["scaleX", "scaleY"] {
            assert!((t[k].as_f64().unwrap() - 512.0 / 1080.0).abs() < 1e-8, "{k}: {}", t[k]);
        }
        edit_clip(&mut s, "Main", "Logo", |c| {
            let (a, b) = (c.source_in, c.source_in + filmcraft_time::Tick(c.duration.0 / 2));
            let m = c.effect_mut("motion").unwrap().params.get_mut("scale").unwrap();
            m.keyframes = vec![filmcraft_project::Keyframe::new(a, ParamValue::Float(100.0)), filmcraft_project::Keyframe::new(b, ParamValue::Float(50.0))];
        });
        let e = save_host_file(&mut s).unwrap_err();
        assert!(e.contains("scale-not-in-cut"), "{e}");
    }

    fn keys_of(s: &Session, seq: &str, clip: &str, effect: &str, param: &str) -> Vec<filmcraft_project::Keyframe> {
        let (id, kind, ti, ci) = find(s, seq, clip);
        let c = &s.project.sequence(id).unwrap().tracks(kind)[ti].items[ci];
        c.effects
            .iter()
            .filter(|e| e.effect == effect)
            .find_map(|e| e.params.get(param))
            .unwrap_or_else(|| panic!("{clip} has no {effect} {param}"))
            .keyframes
            .clone()
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
        assert_eq!(
            op.iter().map(|k| (k.time.0, k.value.clone(), k.interp)).collect::<Vec<_>>(),
            vec![(0, ParamValue::Float(0.0), Interpolation::Linear), (filmcraft_time::TICKS_PER_SECOND, ParamValue::Float(100.0), Interpolation::Linear),],
            "Cut's opacity channel is Film's Opacity keyframes"
        );
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
        assert_eq!(
            (pos[1].value.clone(), pos[1].interp, pos[1].out_influence),
            (ParamValue::Vec2(filmcraft_geom::Vec2::new(1020.0, 500.0)), Interpolation::EaseOut, 0.5)
        );
        let blur = keys_of(&s, "Edit", "Keyed", "gaussian_blur", "blurriness");
        assert_eq!(blur.iter().map(|k| k.interp).collect::<Vec<_>>(), vec![Interpolation::Linear, Interpolation::Hold, Interpolation::Linear]);
        let (id, kind, ti, ci) = find(&s, "Edit", "Keyed sound");
        assert_eq!(s.project.sequence(id).unwrap().tracks(kind)[ti].items[ci].gain_db, -1.0, "Cut's volume is Film's gain");
        let lvl = keys_of(&s, "Edit", "Keyed sound", "volume", "level");
        assert_eq!(lvl.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), vec![ParamValue::Float(-2.0), ParamValue::Float(-8.0)]);
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "unedited, Cut's edit comes back value for value");
    }

    /// files/989 part 2 ("Masks, both ways"; tool choice: Cut edits the masks on Film's OWN clips): Cut's edit of Film's masked clip (the
    /// OpenCut fork's `cut-masked-film.otio`: the ellipse's feather 30, the text "CUT", the open path removed, a rectangle Cut drew added)
    /// opens in Film as Film's own masks, nothing locked or carried, and saves back value for value. Cut's rectangle is Film's path of the
    /// same corners in clip pixels, its feather measured in the canvas's pixels as Cut measures its own.
    #[test]
    fn a_cut_edit_of_film_s_masks_opens_in_film_as_film_s_own() {
        use filmcraft_project::{MaskMode, MaskUnits};
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-masked-film.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "Masks.otio", bytes).unwrap_or_else(|e| panic!("{e}"));
        let (id, kind, ti, ci) = find(&s, "Edit", "Masked");
        let c = &s.project.sequence(id).unwrap().tracks(kind)[ti].items[ci];
        let masks = &c.effects.iter().find(|e| e.effect == "opacity").unwrap().masks;
        assert_eq!(masks.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["Oval", "Cut out", "Title", "Rectangle"]);
        assert_eq!(masks[0].feather.value, ParamValue::Float(30.0));
        assert_eq!((masks[1].mode, masks[1].units, masks[1].inverted), (MaskMode::Subtract, MaskUnits::Canvas, true));
        assert_eq!(masks[2].text.as_ref().map(|t| t.content.as_str()), Some("CUT"));
        let ParamValue::Path(rect) = &masks[3].path.value else { panic!("the rectangle has no path") };
        assert_eq!(
            rect.vertices.iter().map(|v| (v.p.x, v.p.y)).collect::<Vec<_>>(),
            vec![(1200.0, 270.0), (1680.0, 270.0), (1680.0, 810.0), (1200.0, 810.0)],
            "Cut's rectangle (centre a quarter right, a quarter wide, half high) on the 1920 x 1080 medium"
        );
        assert_eq!((masks[3].units, masks[3].feather.value.clone(), masks[3].mode), (MaskUnits::Canvas, ParamValue::Float(6.0), MaskMode::Add));
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
        assert_eq!(
            (size[0].out_influence, size[0].out_rise.as_ref().map(|r| (r[0] - 12.0 * k).abs() < 1e-6)),
            (0.25, Some(true)),
            "Cut's sloped handle is Film's: a quarter of the segment, rising 12 of Cut's pixels"
        );
        let text = keys_of(&s, "Main", "Title", "graphic_text", "text");
        assert_eq!(
            text.iter().map(|k| (k.time.0, k.value.clone())).collect::<Vec<_>>(),
            vec![(0, ParamValue::Text("Hello 日本語".into())), (filmcraft_time::TICKS_PER_SECOND, ParamValue::Text("Goodbye".into()))]
        );
        let font = keys_of(&s, "Main", "Title", "graphic_text", "font");
        assert_eq!(
            font.iter().map(|k| (k.time.0, k.value.clone(), k.interp)).collect::<Vec<_>>(),
            vec![
                (0, ParamValue::Text("Inter".into()), Interpolation::Hold),
                (filmcraft_time::TICKS_PER_SECOND, ParamValue::Text("Roboto".into()), Interpolation::Hold),
            ],
            "keyed at the start with Cut's own font, which Cut shows before its first key"
        );
        let under = keys_of(&s, "Main", "Title", "graphic_text", "underline");
        let struck = keys_of(&s, "Main", "Title", "graphic_text", "strikethrough");
        assert_eq!(
            (under.iter().map(|k| k.value.clone()).collect::<Vec<_>>(), struck.iter().map(|k| k.value.clone()).collect::<Vec<_>>()),
            (vec![ParamValue::Bool(true), ParamValue::Bool(false)], vec![ParamValue::Bool(false), ParamValue::Bool(true)])
        );
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
        want.pointer_mut(&ch("letterSpacing:value")).unwrap()["keys"]
            .as_array_mut()
            .unwrap()
            .insert(0, serde_json::json!({"id": new_id, "time": 0, "value": 2, "segmentToNext": "linear", "tangentMode": "flat"}));
        // Film's new weight key at half a second; the key Film set at the start for Cut's own weight (bold) is not written as Cut's.
        let normal = want.pointer(&ch("fontWeight:value")).unwrap()["keys"][0].clone();
        want.pointer_mut(&ch("fontWeight:value")).unwrap()["keys"] = serde_json::json!([{"id": "film-fontWeight-0", "time": 60000, "value": "bold"}, normal]);
        assert_eq!(first_difference(&want, &edited, ""), None, "only Film's edits differ, as Cut's channels");
    }

    /// Mona 2026-11-09 (files/989): "Add sRGB channel to Cut" and "Film eases linear too". Every colour channel is drawn alike in both
    /// tools between its keys: Film's keys of Cut's own (linear-light) title colour ease in linear light and are written back in that
    /// space; a colour Film keys anew is written as Cut's `srgb` channel and eased in sRGB by both. The edited file is pinned as
    /// `film-coloured-cut.otio` and the frames Film draws between the keys as `colour-frames.json` (`FILMCRAFT_BLESS=1` writes both); the
    /// OpenCut fork's `editor/animation.test.ts` draws the same file in Cut and compares every frame.
    #[test]
    fn film_and_cut_draw_every_colour_alike_between_keys() {
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-project.otio");
        let mut s = session();
        open_host_file(&mut s, "trip.otio", bytes).unwrap();
        let tps = filmcraft_time::TICKS_PER_SECOND;
        edit_clip(&mut s, "Main", "Title", |c| {
            let l = c.effects.iter_mut().find(|e| e.effect == "graphic_text").unwrap();
            l.params.get_mut("fill_color").unwrap().put_keyframe(filmcraft_time::Tick(0), ParamValue::Color([0.0, 0.5, 1.0, 1.0]));
            let bg = l.params.get_mut("background_color").unwrap();
            bg.put_keyframe(filmcraft_time::Tick(0), ParamValue::Color([1.0, 0.0, 0.0, 1.0]));
            bg.put_keyframe(filmcraft_time::Tick(tps), ParamValue::Color([0.0, 0.0, 1.0, 1.0]));
        });
        let fill = keys_of(&s, "Main", "Title", "graphic_text", "fill_color");
        let bg = keys_of(&s, "Main", "Title", "graphic_text", "background_color");
        assert!(fill.iter().all(|k| k.linear_light) && bg.iter().all(|k| !k.linear_light), "Cut's own colour eases in linear light, Film's new one in sRGB");
        let out = save_host_file(&mut s).unwrap_or_else(|e| panic!("Film refused the save: {e}"));
        let edited: Value = serde_json::from_slice(&out).unwrap();
        let el = "/children/0/tracks/children/1/children/1/metadata/opencut/element/animations";
        assert_eq!(edited.pointer(&format!("{el}/bindings/color/colorSpace")), Some(&serde_json::json!("srgb-linear")), "written in its own space");
        assert_eq!(edited.pointer(&format!("{el}/bindings/background.color/colorSpace")), Some(&serde_json::json!("srgb")));
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../interchange/tests/it/oneos");
        // The frames Film draws, every tenth of the second between the keys (Cut's ticks from the element's start).
        let at = |k: &[filmcraft_project::Keyframe], t: i64| {
            filmcraft_project::Param { value: k[0].value.clone(), keyframes: k.to_vec() }
                .value_at(filmcraft_time::Tick(t * (tps / 120_000)))
                .as_color()
                .unwrap()
        };
        let frames: Vec<Value> =
            (0..=10).map(|i| i * 12_000).map(|t| serde_json::json!({"time": t, "color": at(&fill, t), "background.color": at(&bg, t)})).collect();
        let frames = serde_json::to_string_pretty(&frames).unwrap() + "\n";
        if std::env::var_os("FILMCRAFT_BLESS").is_some() {
            std::fs::write(dir.join("film-coloured-cut.otio"), &out).unwrap();
            std::fs::write(dir.join("colour-frames.json"), &frames).unwrap();
        }
        let pinned: Value =
            serde_json::from_slice(&std::fs::read(dir.join("film-coloured-cut.otio")).expect("film-coloured-cut.otio (FILMCRAFT_BLESS=1 writes it)")).unwrap();
        assert_eq!(first_difference(&pinned, &edited, ""), None, "Film saves the colours as pinned");
        assert_eq!(
            std::fs::read_to_string(dir.join("colour-frames.json")).expect("colour-frames.json (FILMCRAFT_BLESS=1 writes it)"),
            frames,
            "Film draws them as pinned"
        );
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

    /// files/989 (Mona 2026-11-10: "Caption track in the .otio"): Film's captions (`film-captions.otio`) edited in Cut as its caption track,
    /// as the OpenCut fork's `oneos-tests/captions.test.ts` writes them (`cut-captions.otio`): a caption's text changed, one moved a second
    /// later, one added, every caption made green (the style), and a new caption track "Français" (as Cut's SRT import makes one). In Film
    /// they are Film's own captions, nothing locked: the cue data Film wrote kept, Cut's style change Film's style, the new track Film's
    /// caption track; an unedited save is the file value for value, and a caption Film edits is written in the same shape.
    #[test]
    fn cut_s_caption_edits_open_in_film_as_its_own_captions() {
        use filmcraft_time::FrameRate;
        let bytes = include_bytes!("../../interchange/tests/it/oneos/cut-captions.otio");
        let cut: Value = serde_json::from_slice(bytes).unwrap();
        let mut s = session();
        open_host_file(&mut s, "Captions.otio", bytes).unwrap();
        assert_eq!(s.host_shared.locked(), Vec::<String>::new(), "nothing is locked");
        let edit = s.project.items.values().find(|i| i.name == "Edit").unwrap().as_sequence().unwrap();
        let r = FrameRate::FPS_24;
        let at = |sec: f64| filmcraft_time::Tick((sec * filmcraft_time::TICKS_PER_SECOND as f64) as i64);
        assert_eq!(edit.caption_tracks.len(), 2, "Film's track and the one Cut added");
        let (en, fr) = (&edit.caption_tracks[0], &edit.caption_tracks[1]);
        let rows = |t: &filmcraft_project::CaptionTrack| t.captions.iter().map(|c| (c.text.clone(), c.start, c.duration)).collect::<Vec<_>>();
        assert_eq!(
            rows(en),
            vec![
                ("Hello there.".to_string(), at(0.0), at(1.0)),
                ("Two lines,\nedited in Cut.".to_string(), at(1.0), at(1.0)),
                ("After a pause.".to_string(), at(4.0), at(1.0)),
                ("Added in Cut.".to_string(), at(6.0), at(1.5)),
            ]
        );
        let _ = r;
        assert_eq!(
            (en.captions[0].speaker.as_deref(), en.captions[0].cue_id.as_deref(), en.captions[0].settings.as_str()),
            (Some("Ana"), Some("intro"), "line:90% align:start")
        );
        assert_eq!(
            (en.style.color, en.style.size, en.language.as_str()),
            ([0, 255, 0, 255], 48.0, "en-AU"),
            "Cut's colour is Film's style; the rest as Film had it"
        );
        assert_eq!((fr.name.as_str(), fr.language.as_str()), ("Français", "fr"));
        assert_eq!(rows(fr), vec![("Bonjour.".to_string(), at(0.5), at(1.0)), ("Deux.".to_string(), at(2.0), at(1.0))]);
        let saved: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        assert_eq!(first_difference(&cut, &saved, ""), None, "Cut's captions come back from Film value for value");
        // Film edits a caption: written in the same shape, the rest of the file kept.
        {
            let id = s.project.items.values().find(|i| i.name == "Edit").unwrap().id;
            let p = std::sync::Arc::make_mut(&mut s.project);
            let q = p.sequence_mut(id).unwrap();
            q.caption_tracks[1].captions[1].text = "Deux, par Film.".into();
        }
        let again: Value = serde_json::from_slice(&save_host_file(&mut s).unwrap()).unwrap();
        let tl = again["children"].as_array().unwrap().iter().find(|t| t["name"] == "Edit").unwrap();
        let fr_out = tl["tracks"]["children"].as_array().unwrap().iter().find(|t| t["name"] == "Français").unwrap();
        let texts: Vec<&str> = fr_out["children"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["OTIO_SCHEMA"] == "Clip.2")
            .map(|c| c["metadata"]["filmcraft"]["caption"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, ["Bonjour.", "Deux, par Film."]);
        let mut s2 = session();
        open_host_file(&mut s2, "Captions.otio", &serde_json::to_vec(&again).unwrap()).expect("Film reopens its own save");
    }
}
