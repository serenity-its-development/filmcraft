//! OpenTimelineIO: a hand-written plain OTIO document (from the public OTIO schema docs) with gaps,
//! transitions, markers and legacy `Clip.1`; lossless round trips through `metadata.filmcraft`.

use crate::support;

use filmcraft_interchange::{ExportOptions, Format, detect, import};
use filmcraft_media::Generator;
use filmcraft_project::{Keyframe, Label, Marker, MarkerId, MarkerKind, MediaRef, Param, ParamValue, Project, TrackKind, TransitionAlign};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use proptest::prelude::*;
use support::*;

fn rt(v: f64, rate: f64) -> String {
    format!(r#"{{"OTIO_SCHEMA": "RationalTime.1", "rate": {rate}, "value": {v}}}"#)
}
fn tr(s: f64, d: f64, rate: f64) -> String {
    format!(r#"{{"OTIO_SCHEMA": "TimeRange.1", "start_time": {}, "duration": {}}}"#, rt(s, rate), rt(d, rate))
}

fn doc() -> String {
    let r = 24.0;
    format!(
        r#"{{
  "OTIO_SCHEMA": "Timeline.1",
  "name": "River Story",
  "global_start_time": {gst},
  "metadata": {{}},
  "tracks": {{
    "OTIO_SCHEMA": "Stack.1",
    "name": "tracks",
    "children": [
      {{
        "OTIO_SCHEMA": "Track.1",
        "name": "V1",
        "kind": "Video",
        "children": [
          {{"OTIO_SCHEMA": "Gap.1", "name": "", "source_range": {gap}}},
          {{"OTIO_SCHEMA": "Transition.1", "name": "Fade In", "transition_type": "SMPTE_Dissolve", "in_offset": {z}, "out_offset": {t6}}},
          {{
            "OTIO_SCHEMA": "Clip.2",
            "name": "river_a",
            "source_range": {src_a},
            "media_references": {{"DEFAULT_MEDIA": {{"OTIO_SCHEMA": "ExternalReference.1", "target_url": "file:///footage/river%20a.mov", "available_range": {avail_a}}}}},
            "active_media_reference_key": "DEFAULT_MEDIA",
            "markers": [{{"OTIO_SCHEMA": "Marker.2", "name": "kingfisher", "color": "RED", "comment": "", "marked_range": {mk}}}]
          }},
          {{"OTIO_SCHEMA": "Transition.1", "name": "", "transition_type": "SMPTE_Dissolve", "in_offset": {t6}, "out_offset": {t6}}},
          {{
            "OTIO_SCHEMA": "Clip.1",
            "name": "river_b",
            "source_range": {src_b},
            "media_reference": {{"OTIO_SCHEMA": "ExternalReference.1", "target_url": "clips/river_b.mov", "available_range": null}},
            "effects": [{{"OTIO_SCHEMA": "LinearTimeWarp.1", "effect_name": "LinearTimeWarp", "time_scalar": 2.0}}]
          }},
          {{"OTIO_SCHEMA": "Gap.1", "source_range": {gap2}}},
          {{
            "OTIO_SCHEMA": "Clip.2",
            "name": "lost",
            "source_range": {src_c},
            "media_references": {{"DEFAULT_MEDIA": {{"OTIO_SCHEMA": "MissingReference.1", "name": "lost_shot.mov"}}}},
            "active_media_reference_key": "DEFAULT_MEDIA",
            "enabled": false
          }},
          {{"OTIO_SCHEMA": "Transition.1", "name": "Fade Out", "transition_type": "SMPTE_Dissolve", "in_offset": {t6}, "out_offset": {z}}}
        ]
      }},
      {{
        "OTIO_SCHEMA": "Track.1",
        "name": "A1",
        "kind": "Audio",
        "children": [
          {{
            "OTIO_SCHEMA": "Clip.2",
            "name": "ambience",
            "source_range": {src_amb},
            "media_references": {{"DEFAULT_MEDIA": {{"OTIO_SCHEMA": "ExternalReference.1", "target_url": "file:///footage/amb.wav"}}}},
            "active_media_reference_key": "DEFAULT_MEDIA",
            "effects": [{{"OTIO_SCHEMA": "Effect.1", "effect_name": "Reverb"}}]
          }}
        ]
      }}
    ],
    "markers": [{{"OTIO_SCHEMA": "Marker.2", "name": "act two", "color": "BLUE", "comment": "", "marked_range": {smk}}}]
  }}
}}"#,
        gst = rt(86_400.0, r),
        z = rt(0.0, r),
        t6 = rt(6.0, r),
        gap = tr(0.0, 12.0, r),
        avail_a = tr(86_400.0, 2400.0, r),
        src_a = tr(86_448.0, 48.0, r),
        mk = tr(86_460.0, 0.0, r),
        src_b = tr(10.0, 36.0, r),
        gap2 = tr(0.0, 24.0, r),
        src_c = tr(0.0, 30.0, r),
        src_amb = tr(0.0, 150.0, r),
        smk = tr(60.0, 0.0, r),
    )
}

#[test]
fn golden_gaps_and_transitions() {
    let d = doc();
    assert_eq!(detect(d.as_bytes(), Some("otio")), Some(Format::Otio));
    let (imp, rep) = import(d.as_bytes(), Format::Otio, Some("/proj")).unwrap();
    let p = &imp.project;
    let sid = only_seq(&imp);
    let s = p.sequence(sid).unwrap();
    let r = FrameRate::FPS_24;
    assert_eq!(p.item(sid).unwrap().name, "River Story");
    assert_eq!(s.settings.frame_rate, r);
    assert_eq!(s.start_timecode, 86_400);
    let v1 = &s.video_tracks[0];
    assert_eq!(v1.items.len(), 3);
    let (a, b, c) = (&v1.items[0], &v1.items[1], &v1.items[2]);
    // source times are relative to the reference's available range start
    assert_eq!((a.start, a.duration, a.source_in), (r.tick_of(12), r.tick_of(48), r.tick_of(48)));
    assert_eq!(media_key(p, a.item), "/footage/river a.mov");
    assert_eq!(a.markers[0].name, "kingfisher");
    assert_eq!(a.markers[0].start, r.tick_of(60));
    assert_eq!(a.markers[0].color, Label::Rose);
    assert_eq!((b.start, b.duration, b.source_in, b.speed), (r.tick_of(60), r.tick_of(36), r.tick_of(10), 2.0));
    assert_eq!(media_key(p, b.item), "/proj/clips/river_b.mov");
    assert_eq!(c.start, r.tick_of(120));
    assert!(!c.enabled);
    assert!(p.item(c.item).unwrap().as_media().unwrap().offline);
    let ts = &v1.transitions;
    assert_eq!(ts.len(), 3);
    assert_eq!((ts[0].from, ts[0].to, ts[0].start, ts[0].duration, ts[0].align), (None, Some(a.id), r.tick_of(12), r.tick_of(6), TransitionAlign::StartAtCut));
    assert_eq!(
        (ts[1].from, ts[1].to, ts[1].start, ts[1].duration, ts[1].align),
        (Some(a.id), Some(b.id), r.tick_of(54), r.tick_of(12), TransitionAlign::CenterAtCut)
    );
    assert_eq!((ts[2].from, ts[2].to, ts[2].start, ts[2].align), (Some(c.id), None, r.tick_of(144), TransitionAlign::EndAtCut));
    assert!(ts.iter().all(|t| t.effect.effect == "cross_dissolve"), "{rep}");
    let amb = &s.audio_tracks[0].items[0];
    assert_eq!(amb.duration, r.tick_of(150));
    assert!(rep.mentions("Reverb"), "{rep}");
    assert_eq!(s.markers[0].name, "act two");
    assert_eq!(s.markers[0].start, r.tick_of(60));
}

fn project() -> (Project, filmcraft_project::ItemId) {
    let r = FrameRate::FPS_23_976;
    let mut p = Project::new("P");
    p.custom_colors = vec![filmcraft_project::HexColor([18, 171, 205])];
    let a = media(&mut p, "/m/a.mov", true, true, r);
    p.item_mut(a).unwrap().custom_label_color = filmcraft_project::HexColor::parse("#ABCDEF");
    if let Some(m) = p.item_mut(a).and_then(|i| i.as_media_mut()) {
        m.info.start_timecode = Some(86_400);
    }
    let b = media(&mut p, "/m/b.mov", true, true, r);
    let g = generator(&mut p, "Matte", Generator::ColorMatte { color: [0.2, 0.4, 0.6, 1.0] });
    let nest = sequence(&mut p, "Inner", r, false);
    clip(&mut p, nest, TrackKind::Video, 0, b, 0..50, 5);
    let s = sequence(&mut p, "Outer", r, false);
    let v1 = clip(&mut p, s, TrackKind::Video, 0, a, 0..48, 24);
    let a1 = clip(&mut p, s, TrackKind::Audio, 0, a, 0..48, 24);
    link(&mut p, s, &[v1, a1]);
    let v2 = clip(&mut p, s, TrackKind::Video, 0, nest, 48..48 + (40), 3);
    transition(&mut p, s, (TrackKind::Video, 0), "iris_round", (Some(v1), Some(v2)), 40..40 + (16), TransitionAlign::CenterAtCut);
    clip(&mut p, s, TrackKind::Video, 1, g, 10..10 + (20), 0);
    {
        let q = p.sequence_mut(s).unwrap();
        // sub-frame audio position
        let mut ai = q.audio_tracks[0].items[0].clone();
        ai.id = filmcraft_project::ClipId(9999);
        ai.link = None;
        ai.start = Tick(TICKS_PER_SECOND * 3 + TICKS_PER_SECOND / 48_000 * 7);
        ai.duration = Tick(TICKS_PER_SECOND / 48_000 * 33_333);
        ai.gain_db = 4.5;
        q.audio_tracks[1].items.push(ai);
        let (_, it) = q.find_item_mut(v1).unwrap();
        it.label = Label::Teal;
        it.custom_label_color = filmcraft_project::HexColor::parse("#12ABCD");
        let mut op = Param::new(ParamValue::Float(100.0));
        op.keyframes.push(Keyframe::new(r.tick_of(24), ParamValue::Float(0.0)));
        op.keyframes.push(Keyframe::new(r.tick_of(36), ParamValue::Float(100.0)));
        it.effect_mut("opacity").unwrap().params.insert("opacity".into(), op);
        it.effects.push(filmcraft_project::find_effect("gaussian_blur").unwrap().instance());
        it.markers.push(Marker {
            url: String::new(),
            author: "Editor 日本語".into(),
            custom_color: filmcraft_project::HexColor::parse("#FF80C0"),
            id: MarkerId(5),
            start: r.tick_of(30),
            duration: r.tick_of(2),
            name: "m".into(),
            comment: "c".into(),
            kind: MarkerKind::Comment,
            color: Label::Yellow,
        });
        let (_, n) = q.find_item_mut(v2).unwrap();
        n.reverse = true;
        n.speed = 1.5;
        q.video_tracks[1].locked = true;
        q.audio_tracks[1].volume_db = -3.0;
        q.markers.push(Marker {
            url: String::new(),
            author: "Editor 日本語".into(),
            custom_color: filmcraft_project::HexColor::parse("#FF80C0"),
            id: MarkerId(6),
            start: r.tick_of(70),
            duration: Tick::ZERO,
            name: "chap".into(),
            comment: String::new(),
            kind: MarkerKind::Chapter,
            color: Label::Mango,
        });
    }
    (p, s)
}

#[test]
fn lossless_roundtrip_with_filmcraft_metadata() {
    let (p, s) = project();
    let (imp, text, rep) = roundtrip(&p, s, Format::Otio, &ExportOptions::default());
    assert!(!rep.has_warnings(), "{rep}");
    assert!(text.contains("\"OTIO_SCHEMA\": \"Timeline.1\""));
    assert!(text.contains("\"target_url\": \"file:///m/a.mov\""));
    let si = only_seq(&imp);
    let q = &imp.project;
    assert_eq!(q.custom_colors, p.custom_colors);
    assert!(q.items.values().any(|i| i.custom_label_color == filmcraft_project::HexColor::parse("#ABCDEF")));
    let (qs, ps) = (q.sequence(si).unwrap(), p.sequence(s).unwrap());
    for kind in [TrackKind::Video, TrackKind::Audio] {
        assert_eq!(structure(q, si, kind), structure(&p, s, kind), "{text}");
    }
    assert_eq!(links(q, si), links(&p, s));
    assert_eq!(qs.settings, ps.settings);
    assert_eq!(
        qs.markers.iter().map(|m| (&m.name, m.start, m.kind, m.color, m.custom_color, &m.author)).collect::<Vec<_>>(),
        ps.markers.iter().map(|m| (&m.name, m.start, m.kind, m.color, m.custom_color, &m.author)).collect::<Vec<_>>()
    );
    assert!(qs.video_tracks[1].locked);
    assert_eq!(qs.audio_tracks[1].volume_db, -3.0);
    // clip-level fields restored exactly
    let (qa, pa) = (&qs.video_tracks[0].items[0], &ps.video_tracks[0].items[0]);
    assert_eq!(format!("{:?}", qa.effects), format!("{:?}", pa.effects)); // NaN auto points compare unequal
    assert_eq!(qa.label, Label::Teal);
    assert_eq!(qa.custom_label_color, filmcraft_project::HexColor::parse("#12ABCD"));
    assert_eq!(
        qa.markers.iter().map(|m| (&m.name, m.start, m.duration, m.custom_color, &m.author)).collect::<Vec<_>>(),
        pa.markers.iter().map(|m| (&m.name, m.start, m.duration, m.custom_color, &m.author)).collect::<Vec<_>>()
    );
    assert_eq!(qs.audio_tracks[1].items[0].gain_db, 4.5);
    assert_eq!(qs.audio_tracks[1].items[0].start, ps.audio_tracks[1].items[0].start);
    assert_eq!(qs.video_tracks[0].transitions[0].effect.effect, "iris_round");
    // media info and generator restored
    let qa_media = q.item(qa.item).unwrap().as_media().unwrap();
    assert_eq!(qa_media.info, p.item(pa.item).unwrap().as_media().unwrap().info);
    let gm = q.item(qs.video_tracks[1].items[0].item).unwrap().as_media().unwrap();
    assert_eq!(gm.media, MediaRef::Generator(Generator::ColorMatte { color: [0.2, 0.4, 0.6, 1.0] }));
    // nested sequence
    let nested = qs.video_tracks[0].items[1].item;
    assert_eq!(q.item(nested).unwrap().name, "Inner");
    assert_eq!(q.sequence(nested).unwrap().video_tracks[0].items[0].source_in, FrameRate::FPS_23_976.tick_of(5));
    // relative media paths
    let o = ExportOptions { relative_to: Some("/m/otio".into()), ..Default::default() };
    let (bytes, _) = filmcraft_interchange::export(&p, s, Format::Otio, &o).unwrap();
    let t = String::from_utf8(bytes.clone()).unwrap();
    assert!(t.contains("\"target_url\": \"../a.mov\""), "{t}");
    let (imp2, _) = import(&bytes, Format::Otio, Some("/m/otio")).unwrap();
    assert_eq!(structure(&imp2.project, imp2.sequences[0], TrackKind::Video), structure(&p, s, TrackKind::Video));
}

#[test]
fn collections_and_bare_tracks() {
    let one = r#"{"OTIO_SCHEMA": "SerializableCollection.1", "name": "Bundle", "children": [
        {"OTIO_SCHEMA": "Timeline.1", "name": "T1", "tracks": {"OTIO_SCHEMA": "Stack.1", "children": [
            {"OTIO_SCHEMA": "Track.1", "kind": "Video", "children": [
                {"OTIO_SCHEMA": "Clip.2", "name": "x", "source_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 25, "value": 0}, "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 25, "value": 50}},
                 "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "GeneratorReference.1", "generator_kind": "SMPTEBars"}}, "active_media_reference_key": "DEFAULT_MEDIA"}]}]}},
        {"OTIO_SCHEMA": "Timeline.1", "name": "T2", "tracks": {"OTIO_SCHEMA": "Stack.1", "children": []}}
    ]}"#;
    let (imp, _) = import(one.as_bytes(), Format::Otio, None).unwrap();
    assert_eq!(imp.sequences.len(), 2);
    let s = imp.project.sequence(imp.sequences[0]).unwrap();
    assert_eq!(s.settings.frame_rate, FrameRate::FPS_25);
    assert_eq!(s.video_tracks[0].items[0].duration, FrameRate::FPS_25.tick_of(50));
    let m = imp.project.item(s.video_tracks[0].items[0].item).unwrap().as_media().unwrap();
    assert_eq!(m.media, MediaRef::Generator(Generator::BarsAndTone));
    assert!(import(b"{\"OTIO_SCHEMA\": \"Clip.2\"}", Format::Otio, None).is_err());
    assert!(import(b"not json", Format::Otio, None).is_err());
}

/// An item may be at an audio sample rate: a 1.5 s gap written as 72 000 at rate 48 000 is valid
/// OTIO, and so is a global start time at that rate. No sequence runs at 48 000 fps, so these do
/// not set the sequence rate (it was 48 000, and importing the file into a project failed); the
/// items keep their exact times.
#[test]
fn items_at_a_sample_rate_do_not_set_the_sequence_rate() {
    let (sr, fps) = (48_000.0, 25.0);
    for global_start in ["null".to_string(), rt(3600.0 * sr, sr)] {
        let d = format!(
            r#"{{"OTIO_SCHEMA": "Timeline.1", "name": "Synced", "global_start_time": {global_start}, "tracks": {{"OTIO_SCHEMA": "Stack.1", "children": [
                {{"OTIO_SCHEMA": "Track.1", "name": "A", "kind": "Video", "children": [
                    {{"OTIO_SCHEMA": "Gap.1", "source_range": {gap}}},
                    {{"OTIO_SCHEMA": "Clip.2", "name": "cam", "source_range": {cam}, "media_references": {{"DEFAULT_MEDIA": {{"OTIO_SCHEMA": "MissingReference.1"}}}}, "active_media_reference_key": "DEFAULT_MEDIA"}}]}},
                {{"OTIO_SCHEMA": "Track.1", "name": "REC", "kind": "Audio", "children": [
                    {{"OTIO_SCHEMA": "Clip.2", "name": "rec", "source_range": {rec}, "media_references": {{"DEFAULT_MEDIA": {{"OTIO_SCHEMA": "MissingReference.1"}}}}, "active_media_reference_key": "DEFAULT_MEDIA"}}]}}]}}}}"#,
            gap = tr(0.0, 1.5 * sr, sr),
            cam = tr(0.0, 100.0, fps),
            rec = tr(0.0, 10.0 * sr, sr),
        );
        let (imp, rep) = import(d.as_bytes(), Format::Otio, None).unwrap();
        let s = imp.project.sequence(only_seq(&imp)).unwrap();
        assert_eq!(s.settings.frame_rate, FrameRate::FPS_25, "global start {global_start}: {rep}");
        s.settings.validate().unwrap();
        assert_eq!(s.start_timecode, if global_start == "null" { 0 } else { 3600 * 25 });
        let cam = &s.video_tracks[0].items[0];
        assert_eq!((cam.start, cam.duration), (Tick(TICKS_PER_SECOND * 3 / 2), FrameRate::FPS_25.tick_of(100)));
        assert_eq!(s.audio_tracks[0].items[0].duration, Tick(TICKS_PER_SECOND * 10));
    }
}

fn palette_collection(first: serde_json::Value, second: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"OTIO_SCHEMA":"SerializableCollection.1","children":[
        {"OTIO_SCHEMA":"Timeline.1","name":"First","metadata":{"filmcraft":first},"tracks":{"OTIO_SCHEMA":"Stack.1","children":[]}},
        {"OTIO_SCHEMA":"Timeline.1","name":"Second","metadata":{"filmcraft":second},"tracks":{"OTIO_SCHEMA":"Stack.1","children":[]}}
    ]})
}

#[test]
fn review_orphan_palette_names_do_not_hide_retained_names() {
    use filmcraft_project::HexColor;
    use serde_json::json;
    let mut names = serde_json::Map::new();
    for i in 0..70 {
        names.insert(HexColor([0, 0, i]).hex(), json!("Orphan"));
    }
    names.insert("#FFFFFE".into(), json!("Later name"));
    names.insert("#FFFFFF".into(), json!("Retained name"));
    let doc = palette_collection(
        json!({"custom_colors":["#FFFFFE"],"custom_color_names":{"#FFFFFE":"Original name"}}),
        json!({"custom_colors":["#FFFFFF"],"custom_color_names":names}),
    );
    let (imp, _) = import(&serde_json::to_vec(&doc).unwrap(), Format::Otio, None).unwrap();
    assert_eq!(imp.project.custom_color_names.len(), 2);
    assert_eq!(imp.project.custom_color_names[&HexColor([255, 255, 254])], "Original name");
    assert_eq!(imp.project.custom_color_names[&HexColor([255, 255, 255])], "Retained name");
}

#[test]
fn review_generator_preset_labels_survive_otio_round_trip() {
    let mut p = Project::new("Generator labels");
    let s = sequence(&mut p, "Main", FrameRate::FPS_25, false);
    for (index, (kind, label, custom)) in [
        (Generator::BlackVideo, Label::Teal, None),
        (Generator::BarsAndTone, Label::Magenta, filmcraft_project::HexColor::parse("#123456")),
        (Generator::ColorMatte { color: [0.2, 0.4, 0.6, 1.0] }, Label::Mango, None),
    ]
    .into_iter()
    .enumerate()
    {
        let id = generator(&mut p, &format!("Generator {index}"), kind);
        let item = p.item_mut(id).unwrap();
        item.label = label;
        item.custom_label_color = custom;
        clip(&mut p, s, TrackKind::Video, 0, id, (index as i64 * 25)..(index as i64 * 25 + 25), 0);
    }
    let (bytes, _) = filmcraft_interchange::export(&p, s, Format::Otio, &ExportOptions::default()).unwrap();
    let (imp, _) = import(&bytes, Format::Otio, None).unwrap();
    for original in p.items.values().filter(|i| i.as_media().is_some()) {
        let restored = imp.project.items.values().find(|i| i.name == original.name).unwrap();
        assert_eq!(restored.label, original.label, "{}", original.name);
        assert_eq!(restored.custom_label_color, original.custom_label_color);
        assert_eq!(restored.as_media().unwrap().media, original.as_media().unwrap().media);
    }
}

#[test]
fn review_distinct_identical_generators_keep_identity_and_shared_references() {
    let mut p = Project::new("Generator identity");
    let s = sequence(&mut p, "Main", FrameRate::FPS_25, false);
    let mut pairs = Vec::new();
    for (index, kind) in [Generator::BlackVideo, Generator::BarsAndTone, Generator::ColorMatte { color: [0.2, 0.4, 0.6, 1.0] }].into_iter().enumerate() {
        let first = generator(&mut p, &format!("First {index}"), kind.clone());
        let second = generator(&mut p, &format!("Second {index}"), kind);
        p.item_mut(first).unwrap().label = Label::Teal;
        let item = p.item_mut(second).unwrap();
        item.label = Label::Mango;
        item.custom_label_color = filmcraft_project::HexColor::parse("#123456");
        for (offset, id) in [first, second, first].into_iter().enumerate() {
            clip(&mut p, s, TrackKind::Video, 0, id, ((index * 3 + offset) as i64 * 25)..((index * 3 + offset) as i64 * 25 + 25), 0);
        }
        pairs.push((first, second));
    }
    let (imp, _, _) = roundtrip(&p, s, Format::Otio, &ExportOptions::default());
    let clips = &imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items;
    assert_eq!(clips.len(), 9);
    assert_eq!(imp.project.items.values().filter(|i| i.as_media().is_some()).count(), 6);
    for ((first, second), group) in pairs.into_iter().zip(clips.as_chunks::<3>().0) {
        assert_ne!(group[0].item, group[1].item, "distinct source items must remain distinct");
        assert_eq!(group[0].item, group[2].item, "repeated source references must remain shared");
        for (original_id, clip) in [(first, &group[0]), (second, &group[1])] {
            let original = p.item(original_id).unwrap();
            let restored = imp.project.item(clip.item).unwrap();
            assert_eq!(restored.name, original.name);
            assert_eq!(restored.label, original.label);
            assert_eq!(restored.custom_label_color, original.custom_label_color);
            assert_eq!(restored.as_media().unwrap().media, original.as_media().unwrap().media);
        }
    }
}

#[test]
fn review_generators_without_valid_item_ids_keep_legacy_sharing() {
    use serde_json::json;
    let mut p = Project::new("Legacy generators");
    let s = sequence(&mut p, "Main", FrameRate::FPS_25, false);
    let first = generator(&mut p, "Black", Generator::BlackVideo);
    let second = generator(&mut p, "Other Black", Generator::BlackVideo);
    let bars = generator(&mut p, "Bars", Generator::BarsAndTone);
    for (offset, id) in [first, second, first, bars].into_iter().enumerate() {
        clip(&mut p, s, TrackKind::Video, 0, id, (offset as i64 * 25)..(offset as i64 * 25 + 25), 0);
    }
    let (bytes, _) = filmcraft_interchange::export(&p, s, Format::Otio, &ExportOptions::default()).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for identity in [None, Some(json!(null)), Some(json!(-1)), Some(json!("invalid")), Some(json!({})), Some(json!([]))] {
        let mut doc = doc.clone();
        for clip in doc["tracks"]["children"][0]["children"].as_array_mut().unwrap() {
            let meta = clip["media_references"]["DEFAULT_MEDIA"]["metadata"]["filmcraft"].as_object_mut().unwrap();
            meta.remove("item");
            if let Some(ref identity) = identity {
                meta.insert("item".into(), identity.clone());
            }
        }
        let (imp, _) = import(&serde_json::to_vec(&doc).unwrap(), Format::Otio, None).unwrap();
        let clips = &imp.project.sequence(only_seq(&imp)).unwrap().video_tracks[0].items;
        assert_eq!(clips[0].item, clips[1].item);
        assert_eq!(clips[0].item, clips[2].item);
        assert_ne!(clips[0].item, clips[3].item);
    }
}

#[test]
fn review_collection_palettes_union_distinct_colors_and_preserve_first_names() {
    use filmcraft_project::HexColor;
    use serde_json::json;
    let doc = palette_collection(
        json!({"custom_colors":["#112233"],"custom_color_names":{"#112233":"First name"}}),
        json!({"custom_colors":["#445566","#112233"],"custom_color_names":{"#445566":"Second name","#112233":"Later name"}}),
    );
    let (imp, _) = import(&serde_json::to_vec(&doc).unwrap(), Format::Otio, None).unwrap();
    let colors = [HexColor([0x11, 0x22, 0x33]), HexColor([0x44, 0x55, 0x66])];
    assert_eq!(imp.project.custom_colors, colors);
    assert_eq!(imp.project.custom_color_names[&colors[0]], "First name");
    assert_eq!(imp.project.custom_color_names[&colors[1]], "Second name");
    let mut target = Project::new("Existing");
    filmcraft_interchange::merge_into(&mut target, imp, None);
    assert_eq!(target.custom_colors, colors);
    assert_eq!(target.custom_color_names[&colors[0]], "First name");
}

#[test]
fn review_collection_empty_or_absent_palettes_do_not_clear_earlier_colors() {
    use serde_json::json;
    for later in [json!({"custom_colors":[]}), json!({})] {
        let doc = palette_collection(json!({"custom_colors":["#112233"],"custom_color_names":{"#112233":"Saved"}}), later);
        let (imp, _) = import(&serde_json::to_vec(&doc).unwrap(), Format::Otio, None).unwrap();
        let mut target = Project::new("Existing");
        filmcraft_interchange::merge_into(&mut target, imp, None);
        let color = filmcraft_project::HexColor([0x11, 0x22, 0x33]);
        assert_eq!(target.custom_colors, [color]);
        assert_eq!(target.custom_color_names[&color], "Saved");
    }
}

#[test]
fn review_collection_full_palette_keeps_names_with_retained_colors() {
    use filmcraft_project::HexColor;
    use serde_json::json;
    let colors: Vec<_> = (0..64).map(|i| HexColor([0, 0, i])).collect();
    let doc = palette_collection(
        json!({"custom_colors":colors.iter().map(|c|c.hex()).collect::<Vec<_>>(),"custom_color_names":{"#000000":"Original"}}),
        json!({"custom_colors":["#445566","#000001","#000000"],"custom_color_names":{"#445566":"Cannot retain","#000001":"Filled","#000000":"Later"}}),
    );
    let (imp, report) = import(&serde_json::to_vec(&doc).unwrap(), Format::Otio, None).unwrap();
    assert_eq!(imp.project.custom_colors, colors);
    assert_eq!(imp.project.custom_color_names[&colors[0]], "Original");
    assert_eq!(imp.project.custom_color_names[&colors[1]], "Filled");
    assert!(!imp.project.custom_color_names.contains_key(&HexColor([0x44, 0x55, 0x66])));
    assert!(report.to_string().contains("64"), "discarded palette entries must be reported: {report}");
}

#[test]
fn review_collection_palette_deduplicates_before_applying_capacity() {
    use filmcraft_project::HexColor;
    use serde_json::json;
    let mut entries = vec!["#000000".to_owned(); 70];
    entries.extend((0..70).map(|i| HexColor([0, 0, i]).hex()));
    let doc = palette_collection(json!({"custom_colors":entries}), json!({}));
    let (imp, report) = import(&serde_json::to_vec(&doc).unwrap(), Format::Otio, None).unwrap();
    assert_eq!(imp.project.custom_colors, (0..64).map(|i| HexColor([0, 0, i])).collect::<Vec<_>>());
    assert!(report.to_string().contains("64"));
}

#[derive(Clone, Debug)]
struct G {
    track: usize,
    gap: i64,
    dur: i64,
    media: usize,
    src: i64,
    speed: u8,
    enabled: bool,
    tr: u8,
    tr_len: i64,
    audio: bool,
    subframe: i64,
}

fn gen_clip() -> impl Strategy<Value = G> {
    (0usize..3, 0i64..3, 20i64..150, 0usize..3, 0i64..2000, 0u8..5, prop::bool::weighted(0.9), 0u8..6, 1i64..9, any::<bool>(), 0i64..2000).prop_map(
        |(track, gap, dur, media, src, speed, enabled, tr, tr_len, audio, subframe)| G {
            track,
            gap: gap * 5,
            dur,
            media,
            src,
            speed,
            enabled,
            tr,
            tr_len,
            audio,
            subframe,
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn otio_roundtrip(clips in prop::collection::vec(gen_clip(), 1..20), ri in 0usize..5) {
        let rate = [FrameRate::FPS_23_976, FrameRate::FPS_25, FrameRate::FPS_29_97, FrameRate::FPS_59_94, FrameRate::FPS_24][ri];
        let mut p = Project::new("P");
        let ms = [media(&mut p, "/x/one.mov", true, true, rate), media(&mut p, "/x/two.mov", true, true, rate), media(&mut p, "/y/three four.mxf", true, true, rate)];
        let s = sequence(&mut p, "S", rate, rate == FrameRate::FPS_29_97);
        let mut ends = [0i64; 3];
        let mut prev: [Option<filmcraft_project::ClipId>; 3] = [None; 3];
        let mut aend = [Tick::ZERO; 3];
        for g in &clips {
            let t = ends[g.track] + g.gap;
            let v = clip(&mut p, s, TrackKind::Video, g.track, ms[g.media], t..t + (g.dur), g.src);
            {
                let q = p.sequence_mut(s).unwrap();
                let (_, it) = q.find_item_mut(v).unwrap();
                it.enabled = g.enabled;
                match g.speed {
                    1 => it.speed = 2.0,
                    2 => it.speed = 0.5,
                    3 => it.reverse = true,
                    4 => it.frame_hold = Some(rate.tick_of(g.src + 3)),
                    _ => {}
                }
            }
            let d = g.tr_len;
            let adjacent = g.gap == 0 && prev[g.track].is_some();
            match g.tr {
                0 if adjacent => transition(&mut p, s, (TrackKind::Video, g.track), "cross_dissolve", (prev[g.track], Some(v)), t - d / 2..t - d / 2 + (d), TransitionAlign::CenterAtCut),
                1 if adjacent => transition(&mut p, s, (TrackKind::Video, g.track), "dip_to_white", (prev[g.track], Some(v)), t..t + (d), TransitionAlign::StartAtCut),
                2 if adjacent => transition(&mut p, s, (TrackKind::Video, g.track), "push", (prev[g.track], Some(v)), t - d..t - d + (d), TransitionAlign::EndAtCut),
                3 if !adjacent => transition(&mut p, s, (TrackKind::Video, g.track), "cross_dissolve", (None, Some(v)), t..t + (d), TransitionAlign::StartAtCut),
                _ => {}
            }
            // Audio at sample (not frame) positions, linked to the video.
            let at = rate.tick_of(t) + Tick(TICKS_PER_SECOND / 48_000 * g.subframe % rate.frame_duration().0.max(1));
            if g.audio && aend[g.track] <= at {
                let a = clip(&mut p, s, TrackKind::Audio, g.track, ms[g.media], t..t + (g.dur), g.src);
                let q = p.sequence_mut(s).unwrap();
                let (_, ai) = q.find_item_mut(a).unwrap();
                ai.start = at;
                aend[g.track] = ai.end();
                link(&mut p, s, &[v, a]);
            }
            ends[g.track] = t + g.dur;
            prev[g.track] = Some(v);
        }
        let (imp, text, _) = roundtrip(&p, s, Format::Otio, &ExportOptions::default());
        let si = only_seq(&imp);
        prop_assert_eq!(structure(&imp.project, si, TrackKind::Video), structure(&p, s, TrackKind::Video), "{}", text);
        prop_assert_eq!(structure(&imp.project, si, TrackKind::Audio), structure(&p, s, TrackKind::Audio));
        prop_assert_eq!(links(&imp.project, si), links(&p, s));
    }
}

#[test]
fn other_export_formats_report_marker_property_loss() {
    let (p, s) = project();
    for format in [Format::Fcp7Xml, Format::Fcpxml, Format::Edl] {
        let (_, report) = filmcraft_interchange::export(&p, s, format, &ExportOptions::default()).unwrap();
        assert!(report.to_string().contains("marker authors or exact custom colors"), "{format:?}: {report}");
    }
}

#[test]
fn marker_urls_roundtrip_in_otio_and_warn_in_other_formats() {
    let (mut p, s) = project();
    let sequence_url = "https://example.com/sequence?q=日本語#review";
    let clip_url = "https://example.com/clip?take=2";
    {
        let q = p.sequence_mut(s).unwrap();
        q.markers[0].url = sequence_url.into();
        q.markers[0].kind = MarkerKind::WebLink;
        q.video_tracks[0].items[0].markers[0].url = clip_url.into();
        q.video_tracks[0].items[0].markers[0].kind = MarkerKind::WebLink;
    }
    let (imp, _, report) = roundtrip(&p, s, Format::Otio, &ExportOptions::default());
    assert!(!report.has_warnings(), "{report}");
    let imported = imp.project.sequence(only_seq(&imp)).unwrap();
    assert_eq!(imported.markers[0].url, sequence_url);
    assert_eq!(imported.video_tracks[0].items[0].markers[0].url, clip_url);
    assert_eq!(imported.markers[0].comment, p.sequence(s).unwrap().markers[0].comment);
    // Imported metadata remains editable text; it must never become permission to launch a handler.
    let unsafe_url = "javascript:alert(1)";
    p.sequence_mut(s).unwrap().markers[0].url = unsafe_url.into();
    let (untrusted, _, _) = roundtrip(&p, s, Format::Otio, &ExportOptions::default());
    let retained = &untrusted.project.sequence(only_seq(&untrusted)).unwrap().markers[0].url;
    assert_eq!(retained, unsafe_url);
    assert!(filmcraft_project::marker_web_url(retained).is_none());
    p.sequence_mut(s).unwrap().markers[0].url = sequence_url.into();

    // URL loss is reported even if there are no authors, custom colors or saved swatches.
    p.custom_colors.clear();
    p.custom_color_names.clear();
    for item in p.items.values_mut() {
        item.custom_label_color = None;
        match &mut item.kind {
            filmcraft_project::ItemKind::Sequence(seq) => {
                let seq = std::sync::Arc::make_mut(seq);
                for marker in &mut seq.markers {
                    marker.author.clear();
                    marker.custom_color = None;
                }
                for clip in seq.all_tracks_mut().flat_map(|track| &mut track.items) {
                    clip.custom_label_color = None;
                    for marker in &mut clip.markers {
                        marker.author.clear();
                        marker.custom_color = None;
                    }
                }
            }
            filmcraft_project::ItemKind::Media(media) => {
                for marker in &mut media.markers {
                    marker.author.clear();
                    marker.custom_color = None;
                }
            }
            _ => {}
        }
    }
    for format in [Format::Fcp7Xml, Format::Fcpxml, Format::Edl, Format::Aaf, Format::Omf] {
        let (_, report) = filmcraft_interchange::export(&p, s, format, &ExportOptions::default()).unwrap();
        assert!(report.to_string().contains("marker URLs"), "{format:?}: {report}");
    }
}

/// #2195: a relative `target_url` is a URL: `Clip%20A.mov` is the file `Clip A.mov`.
#[test]
fn relative_target_urls_are_percent_decoded() {
    let doc = r#"{"OTIO_SCHEMA": "Timeline.1", "name": "T", "tracks": {"OTIO_SCHEMA": "Stack.1", "children": [
        {"OTIO_SCHEMA": "Track.1", "kind": "Video", "children": [
            {"OTIO_SCHEMA": "Clip.2", "name": "a", "source_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 25, "value": 0}, "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 25, "value": 50}},
             "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "media/Clip%20A.mov"}}, "active_media_reference_key": "DEFAULT_MEDIA"}]}]}}"#;
    let (imp, _) = import(doc.as_bytes(), Format::Otio, Some("/proj")).unwrap();
    let p = &imp.project;
    let clip = &p.sequence(imp.sequences[0]).unwrap().video_tracks[0].items[0];
    assert_eq!(media_key(p, clip.item), "/proj/media/Clip A.mov");
}

/// OneOS (doctrine files/989, Mona 2026-11-08: "if otio cannot save then we need our own format that can encompass everything of both"):
/// does a whole Film project survive `.otio` -> project, value for value? Every field of `Project` is set; the timeline is written and
/// read back; each field is compared with its ids left out (an import renumbers them). The test pins the MEASURED list of what the OTIO
/// writer does not carry today, so the list can only shrink (each needs a home under `metadata.filmcraft` before Film can edit `.otio`).
#[test]
fn oneos_a_whole_film_project_through_otio_value_for_value() {
    use serde_json::{Value, json};
    let (mut p, s) = project();
    let r = FrameRate::FPS_23_976;
    p.notes = "Call the client about reel 2".into();
    p.custom_color_names.insert(filmcraft_project::HexColor([18, 171, 205]), "Brand teal".into());
    p.luts.push(serde_json::from_value(json!({"id": "lut1", "name": "Warm", "source_path": null, "format": "cube", "text": "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n"})).unwrap());
    let bin = p.add_bin("Selects", None);
    let sb = filmcraft_project::BinId(p.alloc_id());
    p.search_bins.push(filmcraft_project::SearchBin { id: sb, name: "Interviews".into(), query: Default::default() });
    let unused = media(&mut p, "/m/unused.mov", true, true, r);
    let a = p.items.values().find(|i| i.name.contains("a.mov") || i.name == "a").map(|i| i.id).unwrap();
    p.transcripts
        .insert(a, std::sync::Arc::new(filmcraft_project::Transcript { language: "en".into(), source: "manual".into(), speakers: vec![], words: vec![] }));
    p.source_graphics.insert(a, Default::default());
    p.narrations.insert(
        unused,
        filmcraft_project::Narration {
            text: "Hello".into(),
            language: "en-US".into(),
            voice: "v".into(),
            pitch: Default::default(),
            pace: 1.0,
            speech_duration: r.tick_of(24),
        },
    );
    let other = sequence(&mut p, "Another timeline", r, false);
    let _ = (bin, other);

    let _ = s;
    let (bytes, rep) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).expect("export the whole project");
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("\"OTIO_SCHEMA\": \"SerializableCollection.1\""));
    assert_eq!(
        text.matches("\"OTIO_SCHEMA\": \"Timeline.1\"").count(),
        p.items.values().filter(|i| i.as_sequence().is_some()).count(),
        "one Timeline per sequence"
    );
    let (imp, rep2) = import(&bytes, Format::Otio, None).expect("import the whole project");
    assert!(!rep2.has_warnings(), "{rep2}");
    let q = &imp.project;
    // Value for value: the whole project, ids included (NaN auto points compare as their JSON).
    assert_eq!(serde_json::to_value(q).unwrap(), serde_json::to_value(&p).unwrap());
    let names = |p: &Project, f: &dyn Fn(&filmcraft_project::ProjectItem) -> bool| {
        let mut v: Vec<String> = p.items.values().filter(|i| f(i)).map(|i| i.name.clone()).collect();
        v.sort();
        v
    };
    let vals = |v: Value| -> Vec<String> {
        let mut out: Vec<String> = match v {
            Value::Object(m) => m.into_iter().map(|(_, x)| x.to_string()).collect(),
            Value::Array(a) => a.into_iter().map(|x| x.to_string()).collect(),
            x => vec![x.to_string()],
        };
        out.sort();
        out
    };
    let mut lost: Vec<&str> = Vec::new();
    let mut check = |field: &'static str, same: bool| {
        if !same {
            lost.push(field)
        }
    };
    check("name", q.name == p.name);
    check("notes", q.notes == p.notes);
    check("custom_colors", q.custom_colors == p.custom_colors);
    check("custom_color_names", q.custom_color_names == p.custom_color_names);
    check("settings", q.settings == p.settings);
    check("luts", q.luts == p.luts);
    check(
        "search_bins",
        serde_json::to_value(&q.search_bins).unwrap().as_array().map(|a| a.len()) == serde_json::to_value(&p.search_bins).unwrap().as_array().map(|a| a.len()),
    );
    check("transcripts", vals(serde_json::to_value(&q.transcripts).unwrap()) == vals(serde_json::to_value(&p.transcripts).unwrap()));
    check("source_graphics", vals(serde_json::to_value(&q.source_graphics).unwrap()) == vals(serde_json::to_value(&p.source_graphics).unwrap()));
    check("narrations", vals(serde_json::to_value(&q.narrations).unwrap()) == vals(serde_json::to_value(&p.narrations).unwrap()));
    check("root bins", {
        let b = |p: &Project| {
            let mut v: Vec<String> = serde_json::to_value(&p.root).unwrap()["bins"].as_array().into_iter().flatten().map(|b| b["name"].to_string()).collect();
            v.sort();
            v
        };
        b(q) == b(&p)
    });
    check("items: media", names(q, &|i| i.as_media().is_some()) == names(&p, &|i| i.as_media().is_some()));
    check("items: sequences", names(q, &|i| i.as_sequence().is_some()) == names(&p, &|i| i.as_sequence().is_some()));
    assert!(!text.is_empty());
    let _ = rep;
    // The measured gap (2026-11-08). An entry leaves this list when the writer carries it.
    assert_eq!(lost, Vec::<&str>::new(), "a whole Film project loses nothing through .otio (files/989: \"Stay on .otio, fix writers\")");
}

/// files/989: a timeline changed by another tool is read, and what is not reattached is SAID (the host refuses rather than lose it).
#[test]
fn oneos_a_whole_project_whose_timeline_another_tool_changed_is_named() {
    let (p, _) = project();
    let (bytes, _) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["children"][0]["name"] = serde_json::json!("Renamed elsewhere");
    let (_, rep) = import(&serde_json::to_vec(&v).unwrap(), Format::Otio, None).unwrap();
    assert!(rep.mentions("changed outside FilmCraft"), "{rep}");
}

/// files/989, the Film -> Cut -> Film proof on a file FILM wrote (not a hand-built one): this project, written by Film's whole-project writer,
/// is `tests/it/oneos/film-written.otio`, the very file the OpenCut fork's tests open (its `oneos-tests/fixtures/film-written.otio` is a copy,
/// byte for byte). Two sequences; OneOS media; a plain track Cut edits, a track with a transition Cut carries locked, linked audio.
/// `FILMCRAFT_BLESS=1` writes the golden file; otherwise the writer must give it again, value for value.
#[test]
fn oneos_film_writes_the_file_cut_reads() {
    use serde_json::Value;
    let r = FrameRate::FPS_24;
    let mut p = Project::new("Trip");
    let a = media(&mut p, "oneos:file/01jabcdefghjkmnpqrstvwxyz0?version=01jabcdefghjkmnpqrstvwxyz1", true, true, r);
    let b = media(&mut p, "oneos:file/01jabcdefghjkmnpqrstvwxyz2?version=01jabcdefghjkmnpqrstvwxyz3", true, false, r);
    let s = sequence(&mut p, "Edit", r, false);
    let v1 = clip(&mut p, s, TrackKind::Video, 0, a, 0..48, 24);
    let a1 = clip(&mut p, s, TrackKind::Audio, 0, a, 0..48, 24);
    link(&mut p, s, &[v1, a1]);
    clip(&mut p, s, TrackKind::Video, 0, b, 60..120, 0);
    let x = clip(&mut p, s, TrackKind::Video, 1, a, 0..24, 0);
    let y = clip(&mut p, s, TrackKind::Video, 1, b, 24..48, 10);
    transition(&mut p, s, (TrackKind::Video, 1), "cross_dissolve", (Some(x), Some(y)), 20..28, TransitionAlign::CenterAtCut);
    let alt = sequence(&mut p, "Alt", r, false);
    clip(&mut p, alt, TrackKind::Video, 0, b, 0..30, 0);
    p.notes = "Call the client".into();
    let (bytes, _) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).expect("Film writes the whole project");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/it/oneos/film-written.otio");
    if std::env::var_os("FILMCRAFT_BLESS").is_some() {
        std::fs::write(path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).expect("the golden Film-written file (FILMCRAFT_BLESS=1 writes it)");
    assert_eq!(
        serde_json::from_slice::<Value>(&golden).unwrap(),
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        "Film writes the shared file as it is pinned"
    );
}

/// files/989 step 2 (Mona 2026-11-09: "yes, so this becomes a tool choice not a file type choicer"): Film's own clips ANIMATED, in Film's
/// shape, the file the OpenCut fork's `oneos-tests/film-keys.test.ts` opens in Cut and edits there (pinned there as
/// `fixtures/film-keyed.otio`). One video clip whose Motion position (Linear, then an Ease Out), uniform scale (Bezier both ways), rotation
/// (static), Opacity (a Hold, then Linear) and a Gaussian Blur's blurriness (Linear) are keyframed, and an audio clip whose Volume level is.
/// `FILMCRAFT_BLESS=1` writes the golden file; otherwise the writer must give it again, value for value.
#[test]
fn oneos_film_writes_its_keyframes_for_cut() {
    use filmcraft_geom::Vec2;
    use filmcraft_project::{Interpolation, find_effect};
    use serde_json::Value;
    let r = FrameRate::FPS_24;
    let mut p = Project::new("Keys");
    let a = media(&mut p, "oneos:file/01jabcdefghjkmnpqrstvwxyz0?version=01jabcdefghjkmnpqrstvwxyz1", true, true, r);
    let s = sequence(&mut p, "Edit", r, false);
    let v = clip(&mut p, s, TrackKind::Video, 0, a, 0..48, 24);
    let au = clip(&mut p, s, TrackKind::Audio, 0, a, 0..48, 24);
    let (w, h) = {
        let q = p.sequence(s).unwrap();
        (q.settings.width as f64, q.settings.height as f64)
    };
    let key = |f: i64, value: ParamValue, interp: Interpolation, out_influence: f64, in_influence: f64| Keyframe {
        time: r.tick_of(f),
        value,
        interp,
        out_influence,
        in_influence,
        out_rise: None,
        in_rise: None,
        linear_light: false,
    };
    let third = 1.0 / 3.0;
    {
        let q = p.sequence_mut(s).unwrap();
        let ti = q.video_tracks[0].items.iter_mut().find(|i| i.id == v).unwrap();
        ti.name = "Keyed".into();
        let motion = ti.effects.iter_mut().find(|e| e.effect == "motion").unwrap();
        motion.params.insert(
            "position".into(),
            Param {
                value: ParamValue::Vec2(Vec2::new(w / 2.0 + 100.0, h / 2.0)),
                keyframes: vec![
                    key(0, ParamValue::Vec2(Vec2::new(w / 2.0 + 100.0, h / 2.0)), Interpolation::Linear, third, third),
                    key(12, ParamValue::Vec2(Vec2::new(w / 2.0 + 40.0, h / 2.0 - 40.0)), Interpolation::EaseOut, 0.5, third),
                    key(24, ParamValue::Vec2(Vec2::new(w / 2.0, h / 2.0)), Interpolation::Linear, third, third),
                ],
            },
        );
        motion.params.insert(
            "scale".into(),
            Param {
                value: ParamValue::Float(100.0),
                keyframes: vec![
                    key(0, ParamValue::Float(100.0), Interpolation::Bezier, 0.25, third),
                    key(24, ParamValue::Float(150.0), Interpolation::Bezier, third, 0.75),
                ],
            },
        );
        motion.params.insert("rotation".into(), Param::new(ParamValue::Float(15.0)));
        let opacity = ti.effects.iter_mut().find(|e| e.effect == "opacity").unwrap();
        opacity.params.insert(
            "opacity".into(),
            Param {
                value: ParamValue::Float(100.0),
                keyframes: vec![
                    key(0, ParamValue::Float(0.0), Interpolation::Hold, third, third),
                    key(6, ParamValue::Float(50.0), Interpolation::Linear, third, third),
                    key(18, ParamValue::Float(100.0), Interpolation::Linear, third, third),
                ],
            },
        );
        let mut blur = find_effect("gaussian_blur").unwrap().instance();
        blur.params.insert(
            "blurriness".into(),
            Param {
                value: ParamValue::Float(10.0),
                keyframes: vec![
                    key(0, ParamValue::Float(0.0), Interpolation::Linear, third, third),
                    key(24, ParamValue::Float(20.0), Interpolation::Linear, third, third),
                ],
            },
        );
        ti.effects.push(blur);
        let ta = q.audio_tracks[0].items.iter_mut().find(|i| i.id == au).unwrap();
        ta.name = "Keyed sound".into();
        ta.gain_db = -3.0;
        let vol = ta.effects.iter_mut().find(|e| e.effect == "volume").unwrap();
        vol.params.insert(
            "level".into(),
            Param {
                value: ParamValue::Float(0.0),
                keyframes: vec![
                    key(0, ParamValue::Float(0.0), Interpolation::Linear, third, third),
                    key(24, ParamValue::Float(-6.0), Interpolation::Linear, third, third),
                ],
            },
        );
    }
    let (bytes, _) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).expect("Film writes the whole project");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/it/oneos/film-keyed.otio");
    if std::env::var_os("FILMCRAFT_BLESS").is_some() {
        std::fs::write(path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).expect("the golden Film-keyed file (FILMCRAFT_BLESS=1 writes it)");
    assert_eq!(serde_json::from_slice::<Value>(&golden).unwrap(), serde_json::from_slice::<Value>(&bytes).unwrap(), "Film writes its keyframes as pinned");
}

/// files/989 step 2d (Mona: "Build linked audio, lock multicam"): Film's MULTI-CAMERA clip as Film writes it, the file the OpenCut fork's
/// `oneos-tests/film-multicam.test.ts` opens in Cut (pinned there as `fixtures/film-multicam.otio`), where the track holding it is locked,
/// named as Film's multicam clip, and carried untouched. "Cams" is a multi-camera source sequence of two angles (a video track each);
/// "Edit" holds it on its first video track as a multicam clip showing angle 2, and a plain clip on its second video track. The nested
/// stack names the source's cameras (`metadata.filmcraft.multicam`) and the clip its angle (`metadata.filmcraft.clip.multicam`).
/// `FILMCRAFT_BLESS=1` writes the golden file; otherwise the writer must give it again, value for value, and Film reads it back whole.
#[test]
fn oneos_film_writes_its_multicam_clip_for_cut() {
    use filmcraft_project::{Camera, MulticamAudio, MulticamSel, MulticamSource};
    use serde_json::Value;
    let r = FrameRate::FPS_24;
    let mut p = Project::new("Multicam");
    let a = media(&mut p, "oneos:file/01jabcdefghjkmnpqrstvwxyz0?version=01jabcdefghjkmnpqrstvwxyz1", true, false, r);
    let b = media(&mut p, "oneos:file/01jabcdefghjkmnpqrstvwxyz2?version=01jabcdefghjkmnpqrstvwxyz3", true, false, r);
    let cams = sequence(&mut p, "Cams", r, false);
    clip(&mut p, cams, TrackKind::Video, 0, a, 0..48, 0);
    clip(&mut p, cams, TrackKind::Video, 1, b, 0..48, 0);
    {
        let q = p.sequence_mut(cams).unwrap();
        let (t0, t1) = (q.video_tracks[0].id, q.video_tracks[1].id);
        let cam = |name: &str, t, source| Camera { name: name.into(), video_track: Some(t), audio_tracks: Vec::new(), enabled: true, source: Some(source) };
        q.multicam = Some(MulticamSource { cameras: vec![cam("Wide", t0, a), cam("Close", t1, b)], audio: MulticamAudio::Camera1, sync: "in".into() });
    }
    let s = sequence(&mut p, "Edit", r, false);
    let mc = clip(&mut p, s, TrackKind::Video, 0, cams, 0..48, 0);
    clip(&mut p, s, TrackKind::Video, 1, a, 0..24, 12);
    p.sequence_mut(s).unwrap().find_item_mut(mc).unwrap().1.multicam = Some(MulticamSel { enabled: true, angle: 1 });
    let (bytes, _) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).expect("Film writes the whole project");
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    let edit = doc["children"].as_array().unwrap().iter().find(|t| t["name"] == "Edit").expect("the Edit timeline");
    let stack = &edit["tracks"]["children"][0]["children"][0];
    assert_eq!(stack["OTIO_SCHEMA"], "Stack.1", "the multicam clip is a nested stack");
    assert_eq!(stack["metadata"]["filmcraft"]["clip"]["multicam"], serde_json::json!({"enabled": true, "angle": 1}), "the clip names its angle");
    let names: Vec<&str> = stack["metadata"]["filmcraft"]["multicam"]["cameras"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Wide", "Close"], "the source names its cameras");
    let (back, _) = import(&bytes, Format::Otio, None).expect("Film reads its file");
    assert_eq!(serde_json::to_value(&back.project).unwrap(), serde_json::to_value(&p).unwrap(), "Film reads its multicam back whole");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/it/oneos/film-multicam.otio");
    if std::env::var_os("FILMCRAFT_BLESS").is_some() {
        std::fs::write(path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).expect("the golden Film multicam file (FILMCRAFT_BLESS=1 writes it)");
    assert_eq!(serde_json::from_slice::<Value>(&golden).unwrap(), doc, "Film writes its multicam clip as pinned");
}

/// files/989 part 2 ("Masks, both ways", tool choice: Cut edits the masks on Film's OWN clips): Film's clip MASKED, in Film's shape, the
/// file the OpenCut fork's `oneos-tests/film-own-masks.test.ts` opens in Cut and edits there (pinned there as `fixtures/film-masked.otio`).
/// One video clip of a 1920 x 1080 medium whose Opacity holds an ellipse (Film's own, feathered in clip pixels), a polygon that subtracts
/// (inverted, half opaque, grown by 8, its feather in the canvas's pixels), an open path that is off (mode None) and a text mask, every
/// kind of mask Film draws. `FILMCRAFT_BLESS=1` writes the golden file; otherwise the writer must give it again, value for value.
#[test]
fn oneos_film_writes_its_masks_for_cut() {
    use filmcraft_geom::Vec2;
    use filmcraft_project::{Mask, MaskMode, MaskPath, MaskText, MaskUnits, MaskVertex, TrackMethod};
    use serde_json::Value;
    let r = FrameRate::FPS_24;
    let mut p = Project::new("Masks");
    let a = media(&mut p, "oneos:file/01jabcdefghjkmnpqrstvwxyz0?version=01jabcdefghjkmnpqrstvwxyz1", true, true, r);
    let s = sequence(&mut p, "Edit", r, false);
    let v = clip(&mut p, s, TrackKind::Video, 0, a, 0..48, 24);
    {
        let q = p.sequence_mut(s).unwrap();
        let ti = q.video_tracks[0].items.iter_mut().find(|i| i.id == v).unwrap();
        ti.name = "Masked".into();
        let opacity = ti.effects.iter_mut().find(|e| e.effect == "opacity").unwrap();
        let oval = Mask::new("Oval", MaskPath::ellipse(Vec2::new(960.0, 540.0), Vec2::new(480.0, 300.0)));
        let mut cut = Mask::new("Cut out", MaskPath::polygon(&[Vec2::new(200.0, 100.0), Vec2::new(700.0, 150.0), Vec2::new(450.0, 600.0)]));
        cut.mode = MaskMode::Subtract;
        cut.inverted = true;
        cut.opacity = Param::new(ParamValue::Float(50.0));
        cut.expansion = Param::new(ParamValue::Float(8.0));
        cut.feather = Param::new(ParamValue::Float(20.0));
        cut.units = MaskUnits::Canvas;
        cut.track_method = TrackMethod::Position;
        let mut line = Mask::new(
            "Line",
            MaskPath {
                vertices: vec![
                    MaskVertex::corner(Vec2::new(100.0, 900.0)),
                    MaskVertex::smooth(Vec2::new(900.0, 800.0), Vec2::new(120.0, -40.0)),
                    MaskVertex::corner(Vec2::new(1700.0, 950.0)),
                ],
                closed: false,
            },
        );
        line.mode = MaskMode::None;
        let mut title = MaskText::new("FILM", [0.0, 0.0, 1920.0, 1080.0]);
        title.size = 30.0;
        title.center = Vec2::new(0.1, -0.2);
        title.bold = true;
        let words = Mask::text("Title", title);
        opacity.masks = vec![oval, cut, line, words];
    }
    let (bytes, _) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).expect("Film writes the whole project");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/it/oneos/film-masked.otio");
    if std::env::var_os("FILMCRAFT_BLESS").is_some() {
        std::fs::write(path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).expect("the golden Film-masked file (FILMCRAFT_BLESS=1 writes it)");
    assert_eq!(serde_json::from_slice::<Value>(&golden).unwrap(), serde_json::from_slice::<Value>(&bytes).unwrap(), "Film writes its masks as pinned");
}

/// files/989 (Mona 2026-11-10: "Caption track in the .otio"): Film's CAPTIONS as Film writes them, the file the OpenCut fork's
/// `oneos-tests/captions.test.ts` opens in Cut (pinned there as `fixtures/film-captions.otio`) and edits as its caption track. "Edit" holds a
/// picture and a caption track "English" of three captions (one with a speaker, a WebVTT cue id and settings, one of two lines with markup),
/// its style not Film's default. Each caption track is a `Track.1` after the media tracks; each caption a `Clip.2` of a `GeneratorReference.1`
/// of kind `FilmCraft.caption` named by its text, the caption under `metadata.filmcraft.caption`, the track's style, format and language under
/// `metadata.filmcraft.caption_track`. Film reads it back whole, and a timeline read alone (no project snapshot) still holds the captions.
/// `FILMCRAFT_BLESS=1` writes the golden file; otherwise the writer must give it again, value for value.
#[test]
fn oneos_film_writes_its_captions_for_cut() {
    use filmcraft_project::{Caption, CaptionAnchor, CaptionFormat, CaptionTrack, ClipId, TrackId};
    use serde_json::Value;
    let r = FrameRate::FPS_24;
    let mut p = Project::new("Captions");
    let a = media(&mut p, "oneos:file/01jabcdefghjkmnpqrstvwxyz0?version=01jabcdefghjkmnpqrstvwxyz1", true, false, r);
    let s = sequence(&mut p, "Edit", r, false);
    clip(&mut p, s, TrackKind::Video, 0, a, 0..96, 0);
    let mut ct = CaptionTrack::new(TrackId(p.alloc_id()), "English".into(), CaptionFormat::Subtitle);
    ct.language = "en-AU".into();
    ct.style.size = 48.0;
    ct.style.color = [255, 230, 0, 255];
    ct.style.anchor = CaptionAnchor::Top;
    ct.style.background = false;
    let sec = |n: i64| r.tick_of(n * 24);
    let mut cap = |start: i64, dur: i64, text: &str| Caption {
        id: ClipId(p.alloc_id()),
        start: sec(start),
        duration: sec(dur),
        text: text.into(),
        speaker: None,
        cue_id: None,
        settings: String::new(),
    };
    let mut first = cap(0, 1, "Hello there.");
    first.speaker = Some("Ana".into());
    first.cue_id = Some("intro".into());
    first.settings = "line:90% align:start".into();
    let second = cap(1, 1, "Two lines,\n<i>the second in italics</i>.");
    let third = cap(3, 1, "After a pause.");
    ct.captions = vec![first, second, third];
    p.sequence_mut(s).unwrap().caption_tracks.push(ct);
    let (bytes, _) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).expect("Film writes the whole project");
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    let edit = doc["children"].as_array().unwrap().iter().find(|t| t["name"] == "Edit").expect("the Edit timeline");
    let tracks = edit["tracks"]["children"].as_array().unwrap();
    let caps = tracks.last().unwrap();
    assert_eq!((caps["OTIO_SCHEMA"].as_str(), caps["kind"].as_str(), caps["name"].as_str()), (Some("Track.1"), Some("Video"), Some("English")));
    assert_eq!(caps["metadata"]["filmcraft"]["caption_track"]["language"], "en-AU");
    assert_eq!(caps["metadata"]["filmcraft"]["caption_track"]["style"]["size"], 48.0);
    let kids = caps["children"].as_array().unwrap();
    let kinds: Vec<&str> = kids.iter().map(|c| c["OTIO_SCHEMA"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["Clip.2", "Clip.2", "Gap.1", "Clip.2"], "captions placed by gaps");
    assert_eq!(kids[0]["media_references"]["DEFAULT_MEDIA"]["generator_kind"], "FilmCraft.caption");
    assert_eq!(kids[0]["name"], "Hello there.", "a caption's clip is named by its text");
    assert_eq!(kids[0]["metadata"]["filmcraft"]["caption"]["speaker"], "Ana");
    let (back, _) = import(&bytes, Format::Otio, None).expect("Film reads its file");
    assert_eq!(serde_json::to_value(&back.project).unwrap(), serde_json::to_value(&p).unwrap(), "Film reads its captions back whole");
    // The timeline alone (another tool's view: no project snapshot) still holds the captions.
    let alone = serde_json::to_vec(edit).unwrap();
    let (one, _) = import(&alone, Format::Otio, None).expect("Film reads the timeline alone");
    let q = one.project.items.values().find_map(|i| i.as_sequence()).expect("a sequence");
    let read = &q.caption_tracks[0];
    let want = &p.sequence(s).unwrap().caption_tracks[0];
    assert_eq!((read.name.as_str(), read.language.as_str(), &read.style), (want.name.as_str(), want.language.as_str(), &want.style));
    let texts = |t: &CaptionTrack| {
        t.captions.iter().map(|c| (c.start, c.duration, c.text.clone(), c.speaker.clone(), c.cue_id.clone(), c.settings.clone())).collect::<Vec<_>>()
    };
    assert_eq!(texts(read), texts(want), "each caption at its time, with its text, speaker, cue id and settings");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/it/oneos/film-captions.otio");
    if std::env::var_os("FILMCRAFT_BLESS").is_some() {
        std::fs::write(path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).expect("the golden Film captions file (FILMCRAFT_BLESS=1 writes it)");
    assert_eq!(serde_json::from_slice::<Value>(&golden).unwrap(), doc, "Film writes its captions as pinned");
}

/// files/989 (2c, speed and reverse): Film's SPEED as Film writes it, the file the OpenCut fork's `oneos-tests/film-speed.test.ts` opens in
/// Cut (pinned there as `fixtures/film-speed.otio`). On "Edit"'s first video track "Fast" plays at 200% and "Back" at 50% reversed with
/// Maintain Audio Pitch; on its second "Held" holds a frame. Each clip's speed is its one `LinearTimeWarp.1` (a negative scalar is reverse),
/// Maintain Audio Pitch is `metadata.filmcraft.maintain_pitch` (written only when set), a hold is a `FreezeFrame.1`. `FILMCRAFT_BLESS=1`
/// writes the golden file; otherwise the writer must give it again, value for value, and Film reads it back whole.
#[test]
fn oneos_film_writes_its_speed_for_cut() {
    use serde_json::{Value, json};
    let r = FrameRate::FPS_24;
    let mut p = Project::new("Speed");
    let a = media(&mut p, "oneos:file/01jabcdefghjkmnpqrstvwxyz0?version=01jabcdefghjkmnpqrstvwxyz1", true, true, r);
    let s = sequence(&mut p, "Edit", r, false);
    let fast = clip(&mut p, s, TrackKind::Video, 0, a, 0..48, 24);
    let back = clip(&mut p, s, TrackKind::Video, 0, a, 48..96, 200);
    let held = clip(&mut p, s, TrackKind::Video, 1, a, 0..24, 0);
    {
        let q = p.sequence_mut(s).unwrap();
        for (id, name, speed, reverse, pitch) in [(fast, "Fast", 2.0, false, false), (back, "Back", 0.5, true, true)] {
            let ti = q.video_tracks[0].items.iter_mut().find(|i| i.id == id).unwrap();
            (ti.name, ti.speed, ti.reverse, ti.maintain_pitch) = (name.into(), speed, reverse, pitch);
        }
        let ti = q.video_tracks[1].items.iter_mut().find(|i| i.id == held).unwrap();
        ti.name = "Held".into();
        ti.frame_hold = Some(r.tick_of(10));
    }
    let (bytes, _) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).expect("Film writes the whole project");
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    let edit = doc["children"].as_array().unwrap().iter().find(|t| t["name"] == "Edit").expect("the Edit timeline");
    let v1: Vec<&Value> = edit["tracks"]["children"][0]["children"].as_array().unwrap().iter().filter(|c| c["OTIO_SCHEMA"] == "Clip.2").collect();
    let warp = |s: f64| json!([{"OTIO_SCHEMA": "LinearTimeWarp.1", "name": "", "effect_name": "LinearTimeWarp", "time_scalar": s, "metadata": {}}]);
    assert_eq!((&v1[0]["effects"], &v1[1]["effects"]), (&warp(2.0), &warp(-0.5)), "each clip's speed is its one time warp, reverse negative");
    assert_eq!(v1[0]["metadata"]["filmcraft"].get("maintain_pitch"), None, "Maintain Audio Pitch written only when set");
    assert_eq!(v1[1]["metadata"]["filmcraft"]["maintain_pitch"], json!(true));
    let v2 = edit["tracks"]["children"][1]["children"].as_array().unwrap().iter().find(|c| c["OTIO_SCHEMA"] == "Clip.2").unwrap();
    assert_eq!(v2["effects"][0]["OTIO_SCHEMA"], "FreezeFrame.1", "a hold is a freeze frame");
    let (back_p, _) = import(&bytes, Format::Otio, None).expect("Film reads its file");
    assert_eq!(serde_json::to_value(&back_p.project).unwrap(), serde_json::to_value(&p).unwrap(), "Film reads its speeds back whole");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/it/oneos/film-speed.otio");
    if std::env::var_os("FILMCRAFT_BLESS").is_some() {
        std::fs::write(path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).expect("the golden Film speed file (FILMCRAFT_BLESS=1 writes it)");
    assert_eq!(serde_json::from_slice::<Value>(&golden).unwrap(), doc, "Film writes its speeds as pinned");
}

/// files/989 (Mona 2026-11-10: "Simple ones only"): the transitions Film writes for Cut, in Film's own shape: its simple kinds (Cross
/// Dissolve, Dip to Black, Dip to White; Constant Power, Constant Gain), centred on a cut and at a clip's edge, which Cut edits; and kinds
/// Cut does not edit (Iris Round, Exponential Fade), whose tracks Cut keeps locked and carries untouched.
#[test]
fn oneos_film_writes_its_transitions_for_cut() {
    use serde_json::Value;
    let r = FrameRate::FPS_24;
    let mut p = Project::new("Transitions");
    let file = |n: u8| format!("oneos:file/01jabcdefghjkmnpqrstvwxy{n:02}?version=01jabcdefghjkmnpqrstvwxyz1");
    let pic = media(&mut p, &file(10), true, false, r);
    let snd = media(&mut p, &file(11), false, true, r);
    let s = sequence(&mut p, "Edit", r, false);
    let (v, a) = (TrackKind::Video, TrackKind::Audio);
    let name = |p: &mut Project, k: TrackKind, t: usize, id, n: &str| {
        let ti = p.sequence_mut(s).unwrap().tracks_mut(k)[t].items.iter_mut().find(|i| i.id == id).unwrap();
        ti.name = n.into();
    };
    let (ca, cb, cc) = (clip(&mut p, s, v, 0, pic, 0..48, 24), clip(&mut p, s, v, 0, pic, 48..96, 96), clip(&mut p, s, v, 0, pic, 96..144, 168));
    for (id, n) in [(ca, "A"), (cb, "B"), (cc, "C")] {
        name(&mut p, v, 0, id, n);
    }
    transition(&mut p, s, (v, 0), "cross_dissolve", (Some(ca), Some(cb)), 44..52, TransitionAlign::CenterAtCut);
    transition(&mut p, s, (v, 0), "dip_to_black", (Some(cb), Some(cc)), 90..102, TransitionAlign::CenterAtCut);
    let cd = clip(&mut p, s, v, 1, pic, 24..72, 240);
    name(&mut p, v, 1, cd, "D");
    transition(&mut p, s, (v, 1), "dip_to_white", (None, Some(cd)), 24..36, TransitionAlign::StartAtCut);
    transition(&mut p, s, (v, 1), "cross_dissolve", (Some(cd), None), 60..72, TransitionAlign::EndAtCut);
    let (ce, cf) = (clip(&mut p, s, v, 2, pic, 0..48, 24), clip(&mut p, s, v, 2, pic, 48..96, 96));
    name(&mut p, v, 2, ce, "E");
    name(&mut p, v, 2, cf, "F");
    transition(&mut p, s, (v, 2), "iris_round", (Some(ce), Some(cf)), 40..56, TransitionAlign::CenterAtCut);
    let (s1, s2) = (clip(&mut p, s, a, 0, snd, 0..48, 24), clip(&mut p, s, a, 0, snd, 48..96, 96));
    name(&mut p, a, 0, s1, "S1");
    name(&mut p, a, 0, s2, "S2");
    transition(&mut p, s, (a, 0), "constant_power", (Some(s1), Some(s2)), 44..52, TransitionAlign::CenterAtCut);
    transition(&mut p, s, (a, 0), "constant_gain", (Some(s2), None), 84..96, TransitionAlign::EndAtCut);
    let (s3, s4) = (clip(&mut p, s, a, 1, snd, 0..48, 24), clip(&mut p, s, a, 1, snd, 48..96, 96));
    name(&mut p, a, 1, s3, "S3");
    name(&mut p, a, 1, s4, "S4");
    transition(&mut p, s, (a, 1), "exponential_fade", (Some(s3), Some(s4)), 44..52, TransitionAlign::CenterAtCut);
    let (bytes, _) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).expect("Film writes the whole project");
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    let edit = doc["children"].as_array().unwrap().iter().find(|t| t["name"] == "Edit").expect("the Edit timeline");
    let kids = |k: usize| -> Vec<Value> { edit["tracks"]["children"][k]["children"].as_array().unwrap().clone() };
    let schemas = |k: usize| kids(k).iter().map(|c| c["OTIO_SCHEMA"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(schemas(0), ["Clip.2", "Transition.1", "Clip.2", "Transition.1", "Clip.2"], "each transition between the clips at its cut");
    assert_eq!(schemas(1), ["Gap.1", "Transition.1", "Clip.2", "Transition.1"], "a fade in before its clip, a fade out after it");
    let x = &kids(0)[1];
    assert_eq!((x["name"].as_str(), x["transition_type"].as_str()), (Some("Cross Dissolve"), Some("SMPTE_Dissolve")));
    assert_eq!(x["metadata"]["filmcraft"]["effect"]["effect"], "cross_dissolve");
    assert_eq!(kids(0)[3]["transition_type"], "Custom_Transition", "a dip is Film's custom transition");
    let (back, _) = import(&bytes, Format::Otio, None).expect("Film reads its file");
    let tracks = |q: &Project| {
        let seq = q.sequence(q.items.values().find(|i| i.name == "Edit").unwrap().id).unwrap();
        seq.video_tracks
            .iter()
            .chain(&seq.audio_tracks)
            .map(|t| t.transitions.iter().map(|x| (x.effect.effect.clone(), x.start, x.duration, x.align)).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };
    assert_eq!(tracks(&back.project), tracks(&p), "Film reads its transitions back whole");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/it/oneos/film-transitions.otio");
    if std::env::var_os("FILMCRAFT_BLESS").is_some() {
        std::fs::write(path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).expect("the golden Film transitions file (FILMCRAFT_BLESS=1 writes it)");
    assert_eq!(serde_json::from_slice::<Value>(&golden).unwrap(), doc, "Film writes its transitions as pinned");
}

/// OneOS (files/989, 2c clip markers; Mona 2026-11-10's rule of thumb): Film writes a clip's markers in the clip's own OTIO `markers`, at
/// the medium's time. `FILMCRAFT_BLESS=1` writes `film-markers.otio`, which the OpenCut fork opens (its `oneos-tests/film-markers.test.ts`).
#[test]
fn oneos_film_writes_its_clip_markers_for_cut() {
    use serde_json::Value;
    let r = FrameRate::FPS_24;
    let mut p = Project::new("Markers");
    let file = |n: u8| format!("oneos:file/01jabcdefghjkmnpqrstvwxy{n:02}?version=01jabcdefghjkmnpqrstvwxyz1");
    let pic = media(&mut p, &file(10), true, false, r);
    let snd = media(&mut p, &file(11), false, true, r);
    let s = sequence(&mut p, "Edit", r, false);
    let (v, a) = (TrackKind::Video, TrackKind::Audio);
    let ca = clip(&mut p, s, v, 0, pic, 0..96, 24);
    let cb = clip(&mut p, s, v, 1, pic, 24..72, 200);
    let cs = clip(&mut p, s, a, 0, snd, 0..96, 24);
    let mark = |id: u64, at: i64, len: i64, name: &str, kind: MarkerKind, color: Label| Marker {
        url: String::new(),
        author: String::new(),
        custom_color: None,
        id: MarkerId(id),
        start: r.tick_of(at),
        duration: r.tick_of(len),
        name: name.into(),
        comment: format!("{name} comment"),
        kind,
        color,
    };
    {
        let q = p.sequence_mut(s).unwrap();
        let (_, it) = q.find_item_mut(ca).unwrap();
        it.name = "A".into();
        it.markers = vec![mark(901, 30, 0, "Early", MarkerKind::Comment, Label::Yellow), mark(902, 90, 6, "Late", MarkerKind::Chapter, Label::Cerulean)];
        let (_, it) = q.find_item_mut(cb).unwrap();
        it.name = "B".into();
        it.markers = vec![mark(903, 210, 0, "Bee", MarkerKind::Comment, Label::Mango)];
        let (_, it) = q.find_item_mut(cs).unwrap();
        it.name = "S".into();
        it.markers = vec![mark(904, 60, 0, "Beat", MarkerKind::Comment, Label::Forest)];
    }
    let (bytes, _) = filmcraft_interchange::export_project(&p, &ExportOptions::default()).expect("Film writes the whole project");
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    let edit = doc["children"].as_array().unwrap().iter().find(|t| t["name"] == "Edit").expect("the Edit timeline");
    let a_clip = edit["tracks"]["children"][0]["children"].as_array().unwrap().iter().find(|c| c["name"] == "A").unwrap().clone();
    let names = a_clip["markers"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names, ["Early", "Late"], "the clip's markers on the clip");
    assert_eq!(a_clip["markers"][1]["marked_range"]["start_time"]["value"], 90.0, "at the medium's time");
    let (back, _) = import(&bytes, Format::Otio, None).expect("Film reads its file");
    let marks = |q: &Project| {
        let seq = q.sequence(q.items.values().find(|i| i.name == "Edit").unwrap().id).unwrap();
        seq.video_tracks
            .iter()
            .chain(&seq.audio_tracks)
            .flat_map(|t| t.items.iter())
            .map(|i| (i.name.clone(), i.markers.iter().map(|m| (m.name.clone(), m.start, m.duration, m.kind, m.color)).collect::<Vec<_>>()))
            .collect::<Vec<_>>()
    };
    assert_eq!(marks(&back.project), marks(&p), "Film reads its clip markers back whole");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/it/oneos/film-markers.otio");
    if std::env::var_os("FILMCRAFT_BLESS").is_some() {
        std::fs::write(path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).expect("the golden Film clip-markers file (FILMCRAFT_BLESS=1 writes it)");
    assert_eq!(serde_json::from_slice::<Value>(&golden).unwrap(), doc, "Film writes its clip markers as pinned");
}
