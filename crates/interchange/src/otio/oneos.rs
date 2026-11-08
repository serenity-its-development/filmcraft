//! OneOS (doctrine files/989): the ONE video project file Film and Cut both edit.
//!
//! Mona 2026-11-09: "film should be able to edit cut though" and "yes, so this becomes a tool choice not a file type choicer". A shared
//! `.otio` holds Film's whole project (a `SerializableCollection.1` of Film's timelines, the project under `metadata.filmcraft.project`) and
//! Cut's parts (its scenes as timelines with `metadata.opencut.scene`, its tracks with `metadata.opencut.track`, its bookmarks, its project
//! and media under the root's `metadata.opencut`). Film opens ALL of it as Film's own types and edits it:
//!
//! - Film's timelines are read through the project Film keeps; a timeline Cut changed (a trim, a move, a clip Cut added or removed, a
//!   renamed track) is reconciled into that project ([`reconcile`]), the clip Film already had keeping everything of Film's.
//! - Cut's parts become Film entities through Cut's codec ([`super::cut`]) and are written back in Cut's shape.
//! - A save is a three-way merge ([`merge3`]): what Film's rendering did not change since the file was opened is written exactly as the file
//!   held it (every key, every number, every other editor's data); only what Film changed is written as Film renders it now.
//! - The file opens only when Film's rendering of it differs from the file in nothing but what Film may carry or derive ([`first_damage`]):
//!   another editor's metadata, the same time at another rate, Film's own tick mirrors and Film's own project snapshot after a reconcile.
//!   Anything else is a part Film would damage, refused by name.

use std::collections::BTreeSet;
use std::sync::Arc;

use filmcraft_project::{ItemId, ItemKind, MediaRef, Project, TrackKind};
use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Map, Value};

use super::{export, export_project, read_tr, same_value, schema, schema_name, sequences_of};
use crate::common::{MediaSpec, item_media, media_clip, media_info};
use crate::{ExportOptions, ImportOptions, Report, is_oneos_media};

pub use super::cut::{CutParts, Locked};

/// Why a shared file is not opened (or saved) for editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedError {
    Unreadable(String),
    NotRoundTrip(String),
    MediaNotReferenced(String),
}

impl std::fmt::Display for SharedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(s) => write!(f, "file-unreadable: {s}"),
            Self::NotRoundTrip(s) => write!(f, "format-not-round-trip: {s}"),
            Self::MediaNotReferenced(s) => write!(f, "media-not-referenced: {s}"),
        }
    }
}

/// The shared file as Film holds it open.
#[derive(Debug, Clone, Default)]
pub struct Shared {
    /// The file as it was opened (or last saved).
    pub file: Value,
    /// Film's rendering of the project when the file was opened (or last saved).
    pub then: Value,
    /// Cut's parts as Film entities, and what is carried locked.
    pub cut: CutParts,
    /// The project items Film's own part held when the file was opened.
    film_items: BTreeSet<ItemId>,
}

impl Shared {
    /// What Film shows but does not edit, by name (a Cut part Film would damage).
    pub fn locked(&self) -> Vec<String> {
        self.cut.locked.iter().map(|l| l.named()).collect()
    }
}

// ---------------------------------------------------------------------------------------------
// The split: Film's part of the file, and Cut's.
// ---------------------------------------------------------------------------------------------

fn meta<'v>(v: &'v Value, key: &str) -> Option<&'v Value> {
    v.get("metadata").and_then(|m| m.get(key))
}

fn is_timeline(v: &Value) -> bool {
    schema(v) == "Timeline.1"
}

/// A timeline of Cut's: a scene (`metadata.opencut.scene`) Film never wrote.
pub(super) fn is_cut_timeline(v: &Value) -> bool {
    is_timeline(v) && meta(v, "filmcraft").is_none() && meta(v, "opencut").and_then(|o| o.get("scene")).is_some_and(Value::is_object)
}

/// A track of Cut's in a Film timeline: Cut's own track or stacked lanes (`metadata.opencut.track`), Film never wrote it.
pub(super) fn is_cut_track(v: &Value) -> bool {
    matches!(schema_name(v), "Track" | "Stack") && meta(v, "filmcraft").is_none() && meta(v, "opencut").and_then(|o| o.get("track")).is_some_and(Value::is_object)
}

/// One of Cut's bookmarks (a `Marker.2` with `metadata.opencut.bookmark`).
pub(super) fn is_cut_marker(v: &Value) -> bool {
    meta(v, "opencut").and_then(|o| o.get("bookmark")).is_some_and(Value::is_object)
}

/// Cut's parts of the file, set aside before Film's part is read.
#[derive(Debug, Default)]
pub(super) struct Split {
    /// Cut's timelines, each at its index among the root's children.
    pub timelines: Vec<(usize, Value)>,
    /// Cut's tracks in Film's timelines: (the index of Film's timeline among Film's, the index in its stack, the track).
    pub tracks: Vec<(usize, usize, Value)>,
    /// Cut's bookmarks on Film's timelines: (the index of Film's timeline, each bookmark).
    pub markers: Vec<(usize, Value)>,
    /// The root's metadata other than Film's.
    pub root: Map<String, Value>,
}

/// Film's part of `doc` and Cut's.
pub(super) fn split(doc: &Value) -> (Value, Split) {
    let mut film = doc.clone();
    let mut s = Split::default();
    if schema_name(&film) == "SerializableCollection" {
        if let Some(Value::Object(m)) = film.get_mut("metadata") {
            let other: Vec<String> = m.keys().filter(|k| k.as_str() != "filmcraft").cloned().collect();
            for k in other {
                if let Some(v) = m.remove(&k) {
                    s.root.insert(k, v);
                }
            }
        }
        if let Some(Value::Array(children)) = film.get_mut("children") {
            let kept: Vec<Value> = std::mem::take(children)
                .into_iter()
                .enumerate()
                .filter_map(|(i, t)| {
                    if is_cut_timeline(&t) {
                        s.timelines.push((i, t));
                        None
                    } else {
                        Some(t)
                    }
                })
                .collect();
            *children = kept;
        }
    }
    let timelines: Vec<&mut Value> = if is_timeline(&film) {
        vec![&mut film]
    } else {
        match film.get_mut("children") {
            Some(Value::Array(c)) => c.iter_mut().filter(|t| is_timeline(t)).collect(),
            _ => Vec::new(),
        }
    };
    for (fi, tl) in timelines.into_iter().enumerate() {
        let Some(Value::Object(stack)) = tl.get_mut("tracks") else { continue };
        if let Some(Value::Array(tracks)) = stack.get_mut("children") {
            let kept: Vec<Value> = std::mem::take(tracks)
                .into_iter()
                .enumerate()
                .filter_map(|(k, t)| {
                    if is_cut_track(&t) {
                        s.tracks.push((fi, k, t));
                        None
                    } else {
                        Some(t)
                    }
                })
                .collect();
            *tracks = kept;
        }
        if let Some(Value::Array(markers)) = stack.get_mut("markers") {
            let (cut, own): (Vec<Value>, Vec<Value>) = std::mem::take(markers).into_iter().partition(is_cut_marker);
            *markers = own;
            s.markers.extend(cut.into_iter().map(|m| (fi, m)));
        }
    }
    (film, s)
}

// ---------------------------------------------------------------------------------------------
// Film's part: the project Film keeps, with the timelines Cut changed reconciled into it.
// ---------------------------------------------------------------------------------------------

/// Film's part read as Film's project, and whether a timeline was reconciled (Film's snapshot then differs from the file's).
fn film_part(film: &Value, name: &str) -> Result<(Project, bool), SharedError> {
    let not = |s: String| SharedError::NotRoundTrip(s);
    if schema_name(film) == "SerializableCollection" {
        if let Some(embedded) = film.pointer("/metadata/filmcraft/project") {
            let mut project: Project = serde_json::from_value(embedded.clone()).map_err(|e| SharedError::Unreadable(format!("Film's project could not be read: {e}")))?;
            let seqs = sequences_of(&project);
            let children = film.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
            if seqs.len() != children.len() {
                return Err(not(format!("holds {} of Film's timelines where Film's project has {}: a timeline of Film's was added or removed outside Film", children.len(), seqs.len())));
            }
            let mut reconciled = false;
            for (s, child) in seqs.iter().zip(&children) {
                let mut scratch = Report::default();
                let text = export(&project, *s, &ExportOptions::default(), &mut scratch).map_err(|e| SharedError::Unreadable(e.to_string()))?;
                let ours: Value = serde_json::from_str(&text).map_err(|e| SharedError::Unreadable(e.to_string()))?;
                if same_value(&film_view(&ours), &film_view(child)) {
                    continue;
                }
                reconcile(&mut project, *s, child).map_err(|why| not(format!("the timeline \"{}\" {why}", child.get("name").and_then(Value::as_str).unwrap_or(""))))?;
                reconciled = true;
            }
            return Ok((project, reconciled));
        }
        let none = film.get("children").and_then(Value::as_array).is_none_or(|c| c.is_empty());
        if none {
            return Ok((Project::new(name), false));
        }
    }
    let text = serde_json::to_string(film).map_err(|e| SharedError::Unreadable(e.to_string()))?;
    let mut report = Report::default();
    let imported = super::import(&text, &ImportOptions::default(), &mut report).map_err(|e| SharedError::Unreadable(e.to_string()))?;
    if report.has_warnings() {
        let said: Vec<String> = report.warnings().map(|w| w.message.clone()).collect();
        return Err(not(format!("holds what Film cannot represent ({})", said.join("; "))));
    }
    Ok((imported.project, false))
}

/// `v` with every metadata key other than Film's own removed (what Film reads of it).
pub(super) fn film_view(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| {
                    if k == "metadata"
                        && let Value::Object(mm) = x
                    {
                        return (k.clone(), Value::Object(mm.iter().filter(|(kk, _)| kk.as_str() == "filmcraft").map(|(kk, xx)| (kk.clone(), film_view(xx))).collect()));
                    }
                    (k.clone(), film_view(x))
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(film_view).collect()),
        _ => v.clone(),
    }
}

/// The fields of Film's clip Cut edits in Film's own shape (files/989 step 2): read back from the file's clip by `reconcile`.
const CUT_EDITS_OF_A_CLIP: &[&str] = &["gain_db", "effects"];

/// What identifies one of Film's clips across another editor's edit: everything Film reads of it but its place, its trim and its name.
fn clip_signature(c: &Value) -> Value {
    let mut s = film_view(c);
    if let Value::Object(m) = &mut s {
        m.remove("source_range");
        m.remove("name");
        if let Some(Value::Object(f)) = m.get_mut("metadata").and_then(|x| x.get_mut("filmcraft")) {
            f.remove("source_in_ticks");
            f.remove("duration_ticks");
            // What Cut edits of Film's clip in Film's own shape (files/989 step 2): its gain and its effects (look and keyframes).
            for k in CUT_EDITS_OF_A_CLIP {
                f.remove(*k);
            }
        }
    }
    s
}

/// A track's children as (start on the track, source in, duration, the clip); a gap only moves the start.
fn lane(children: &[Value]) -> Result<Vec<(Tick, Tick, Tick, &Value)>, String> {
    let mut out = Vec::new();
    let mut pos = Tick::ZERO;
    for c in children {
        match schema_name(c) {
            "Gap" => pos += c.get("source_range").and_then(read_tr).map(|r| r.1).ok_or("holds a gap without a length")?,
            "Clip" => {
                let (src, dur) = c.get("source_range").and_then(read_tr).ok_or("holds a clip without a source range")?;
                out.push((pos, src, dur, c));
                pos += dur;
            }
            other => return Err(format!("holds a {other} where it was changed outside Film")),
        }
    }
    Ok(out)
}

/// The longest common subsequence of `a` and `b` under `eq`, as index pairs in order.
pub(super) fn lcs<T>(a: &[T], b: &[T], eq: impl Fn(&T, &T) -> bool) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if eq(&a[i], &b[j]) { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if eq(&a[i], &b[j]) {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// The media item of a OneOS medium in `p`, made when `p` has none (a medium another editor put on Film's timeline).
pub(super) fn oneos_media(p: &mut Project, url: &str, name: &str, kind: TrackKind, length: Option<Tick>) -> ItemId {
    if let Some(id) = p.items.values().find(|i| matches!(i.as_media().map(|m| &m.media), Some(MediaRef::File { path }) if path == url)).map(|i| i.id) {
        return id;
    }
    let spec = MediaSpec {
        duration: length,
        video: (kind == TrackKind::Video).then(|| (1920, 1080, filmcraft_time::FrameRate::default())),
        audio: (kind == TrackKind::Audio).then_some((48_000, 2)),
        start_tc: None,
        kind: None,
    };
    let info = media_info(name, &spec);
    p.add_item(name, filmcraft_project::Label::Iris, ItemKind::Media(Box::new(media_clip(MediaRef::File { path: url.to_string() }, info))), None)
}

/// Bring Film's sequence `seq_id` to what the timeline `tl` (Film's, changed by another editor) now says. What Cut can do to a track of
/// Film's is reconciled: a clip moved or trimmed (it keeps everything else of Film's: effects, links, labels, gain...), a clip added (a OneOS
/// medium), a clip removed, a track or the timeline renamed. Anything else is refused, named.
fn reconcile(p: &mut Project, seq_id: ItemId, tl: &Value) -> Result<(), String> {
    let mut scratch = Report::default();
    let ours: Value = serde_json::from_str(&export(p, seq_id, &ExportOptions::default(), &mut scratch).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let (o, e) = (film_view(tl), film_view(&ours));
    for k in ["OTIO_SCHEMA", "global_start_time", "metadata"] {
        if !same_value(o.get(k).unwrap_or(&Value::Null), e.get(k).unwrap_or(&Value::Null)) {
            return Err(format!("had its {k} changed outside Film"));
        }
    }
    let (os, es) = (&o["tracks"], &e["tracks"]);
    for k in ["OTIO_SCHEMA", "source_range", "effects", "markers", "enabled", "metadata"] {
        if !same_value(os.get(k).unwrap_or(&Value::Null), es.get(k).unwrap_or(&Value::Null)) {
            return Err(format!("had its stack's {k} changed outside Film"));
        }
    }
    let empty = Vec::new();
    let ot = os.get("children").and_then(Value::as_array).unwrap_or(&empty);
    let et = es.get("children").and_then(Value::as_array).unwrap_or(&empty);
    if ot.len() != et.len() {
        return Err("had a track of Film's added or removed outside Film".into());
    }
    let mut seq = p.sequence(seq_id).ok_or("is not a sequence of Film's project")?.clone();
    let rate = seq.settings.frame_rate;
    let nv = seq.video_tracks.len();
    for (k, (ok, ek)) in ot.iter().zip(et).enumerate() {
        if same_value(ok, ek) {
            continue;
        }
        let (kind, idx) = if k < nv { (TrackKind::Video, k) } else { (TrackKind::Audio, k - nv) };
        // Cut edits a Film track's mute as Film's own switch (files/989 step 2); nothing else of its metadata.
        let unmuted = |v: &Value| {
            let mut v = v.get("metadata").cloned().unwrap_or(Value::Null);
            if let Some(Value::Object(f)) = v.get_mut("filmcraft") {
                f.remove("muted");
            }
            v
        };
        for f in ["OTIO_SCHEMA", "kind", "source_range", "effects", "markers", "enabled"] {
            if !same_value(ok.get(f).unwrap_or(&Value::Null), ek.get(f).unwrap_or(&Value::Null)) {
                return Err(format!("had the {f} of its track {k} changed outside Film"));
            }
        }
        if !same_value(&unmuted(ok), &unmuted(ek)) {
            return Err(format!("had the metadata of its track {k} changed outside Film"));
        }
        let name = ok.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let track = seq.tracks_mut(kind).get_mut(idx).ok_or("does not hold the track Film wrote")?;
        track.name = name;
        if let Some(m) = ok.pointer("/metadata/filmcraft/muted").and_then(Value::as_bool) {
            track.muted = m;
        }
        let (oc, ec) = (ok.get("children").and_then(Value::as_array).unwrap_or(&empty), ek.get("children").and_then(Value::as_array).unwrap_or(&empty));
        if same_value(&Value::Array(oc.clone()), &Value::Array(ec.clone())) {
            continue;
        }
        if !track.transitions.is_empty() {
            return Err(format!("had the track \"{}\", which holds transitions, changed outside Film", track.name));
        }
        // The file's own clips (with every key: Film reads its mirrors from them), against Film's.
        let raw_children = tl.pointer(&format!("/tracks/children/{k}/children")).and_then(Value::as_array).unwrap_or(&empty);
        let now = lane(raw_children).map_err(|w| format!("had its track \"{}\" changed so it {w}", track.name))?;
        let was = lane(ec).map_err(|w| format!("had its track \"{}\" written so it {w}", track.name))?;
        if was.len() != track.items.len() {
            return Err(format!("had its track \"{}\", which holds a nested sequence Film wrote as a gap, changed outside Film", track.name));
        }
        let (ns, ws): (Vec<Value>, Vec<Value>) = (now.iter().map(|c| clip_signature(c.3)).collect(), was.iter().map(|c| clip_signature(c.3)).collect());
        let pairs = lcs(&ns, &ws, same_value);
        let old = std::mem::take(&mut track.items);
        let mut items = Vec::with_capacity(now.len());
        for (i, (start, src, dur, c)) in now.iter().enumerate() {
            let mut it = match pairs.iter().find(|(a, _)| *a == i) {
                Some(&(_, j)) => old[j].clone(),
                None => {
                    let r = c.pointer("/media_references/DEFAULT_MEDIA").unwrap_or(&Value::Null);
                    let url = r.get("target_url").and_then(Value::as_str).unwrap_or("");
                    if schema_name(r) != "ExternalReference" || !is_oneos_media(url) {
                        return Err(format!("had a clip added to its track \"{}\" whose medium is not a OneOS file", track.name));
                    }
                    let cname = c.get("name").and_then(Value::as_str).unwrap_or("Clip");
                    let item = oneos_media(p, url, r.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(cname), kind, None);
                    p.make_track_item(item, kind, *start, TimeRange::new(*src, *dur), rate).ok_or("could not place a clip Cut added")?
                }
            };
            let origin = item_media(p, crate::common::base_item(p, it.item)).and_then(|m| m.info.start_timecode.map(|f| m.frame_rate().tick_of(f))).unwrap_or(Tick::ZERO);
            it.start = *start;
            it.duration = *dur;
            it.source_in = *src - origin;
            // Film's exact mirrors, when they still say the same time.
            let fc = c.pointer("/metadata/filmcraft");
            if let Some(s) = fc.and_then(|m| m.get("source_in_ticks")).and_then(Value::as_i64)
                && (Tick(s) - it.source_in).abs() < Tick(TICKS_PER_SECOND / 1000)
            {
                it.source_in = Tick(s);
            }
            if let Some(d) = fc.and_then(|m| m.get("duration_ticks")).and_then(Value::as_i64)
                && (Tick(d) - it.duration).abs() < Tick(TICKS_PER_SECOND / 1000)
            {
                it.duration = Tick(d);
            }
            it.name = c.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            it.enabled = c.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            // What Cut edited in Film's own shape: the clip's gain and effects (look and keyframes) are the file's.
            if let Some(g) = fc.and_then(|m| m.get("gain_db")).and_then(Value::as_f64) {
                it.gain_db = g;
            }
            if let Some(e) = fc.and_then(|m| m.get("effects")) {
                it.effects = super::effects_from(Some(e)).ok_or_else(|| format!("had the effects of \"{}\" on its track \"{}\" changed so Film cannot read them", it.name, track.name))?;
            }
            items.push(it);
        }
        for w in items.windows(2) {
            if w[0].end() > w[1].start {
                return Err(format!("had clips on its track \"{}\" made to overlap outside Film", track.name));
            }
        }
        track.items = items;
        track.sort();
    }
    let item = p.item_mut(seq_id).ok_or("is not an item of Film's project")?;
    item.name = tl.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    item.kind = ItemKind::Sequence(Arc::new(seq));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The gate: the file opens only when Film's rendering differs from it in nothing Film would damage.
// ---------------------------------------------------------------------------------------------

fn is_rational_time(v: &Value) -> bool {
    schema(v) == "RationalTime.1"
}

fn same_time(a: &Value, b: &Value) -> bool {
    let f = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    match (f(a, "value"), f(a, "rate"), f(b, "value"), f(b, "rate")) {
        (Some(va), Some(ra), Some(vb), Some(rb)) if ra > 0.0 && rb > 0.0 => {
            let (x, y) = (va * rb, vb * ra);
            (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0)
        }
        _ => false,
    }
}

/// Film's own derived fields: its exact tick mirrors of an OTIO time (the time itself is what is read).
const MIRRORS: &[&str] = &["source_in_ticks", "duration_ticks", "start_ticks"];

/// The JSON pointer of the first place Film's rendering `then` would damage `file` (`None`: it differs only in what Film carries or
/// derives). `snapshot`: Film's project snapshot may differ (a timeline was reconciled into it).
pub fn first_damage(file: &Value, then: &Value, snapshot: bool) -> Option<String> {
    damage(file, then, "", Ctx::Plain, snapshot)
}

#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    Plain,
    /// Inside a `metadata` object.
    Metadata,
    /// Inside Film's own metadata (`metadata.filmcraft`).
    Film,
}

fn damage(file: &Value, then: &Value, at: &str, ctx: Ctx, snapshot: bool) -> Option<String> {
    match (file, then) {
        (Value::Object(f), Value::Object(t)) => {
            if is_rational_time(file) && is_rational_time(then) {
                return (!same_time(file, then)).then(|| at.to_string());
            }
            let mut keys: Vec<&String> = f.keys().chain(t.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let here = format!("{at}/{k}");
                match (f.get(k), t.get(k)) {
                    // Another editor's data, carried.
                    (Some(_), None) if ctx == Ctx::Metadata && k != "filmcraft" => {}
                    // Film's own data the file did not hold (a Cut project Film has not written to).
                    (None, Some(_)) if ctx == Ctx::Metadata && k == "filmcraft" => {}
                    (Some(_), None) | (None, Some(_)) => return Some(here),
                    (Some(a), Some(b)) => {
                        if ctx == Ctx::Film && MIRRORS.contains(&k.as_str()) {
                            continue;
                        }
                        if here == "/metadata/filmcraft/project/next_id" || (snapshot && here == "/metadata/filmcraft/project") {
                            continue;
                        }
                        let inner = if k == "metadata" {
                            Ctx::Metadata
                        } else if ctx == Ctx::Metadata && k == "filmcraft" || ctx == Ctx::Film {
                            Ctx::Film
                        } else {
                            Ctx::Plain
                        };
                        if let Some(d) = damage(a, b, &here, inner, snapshot) {
                            return Some(d);
                        }
                    }
                    (None, None) => {}
                }
            }
            None
        }
        (Value::Array(f), Value::Array(t)) => {
            if f.len() != t.len() {
                return Some(at.to_string());
            }
            f.iter().zip(t).enumerate().find_map(|(i, (a, b))| damage(a, b, &format!("{at}/{i}"), if ctx == Ctx::Film { Ctx::Film } else { Ctx::Plain }, snapshot))
        }
        _ if same_value(file, then) => None,
        _ => Some(if at.is_empty() { "/".into() } else { at.into() }),
    }
}

// ---------------------------------------------------------------------------------------------
// The save: a three-way merge.
// ---------------------------------------------------------------------------------------------

/// The file to write: `file` as opened, with what Film's rendering changed from `then` to `now` (and only that) written as `now` has it.
/// A key the file holds that Film never renders (another editor's data) is kept; a key only Film's rendering holds, unchanged, is not
/// added (the file stays in its own shape).
pub fn merge3(file: &Value, then: &Value, now: &Value) -> Value {
    if same_value(then, now) {
        return file.clone();
    }
    match (file, then, now) {
        (Value::Object(f), Value::Object(t), Value::Object(n)) => {
            let mut out = Map::new();
            for (k, fv) in f {
                match (t.get(k), n.get(k)) {
                    (Some(tv), Some(nv)) => {
                        out.insert(k.clone(), merge3(fv, tv, nv));
                    }
                    (Some(_), None) => {}
                    (None, Some(nv)) => {
                        out.insert(k.clone(), nv.clone());
                    }
                    (None, None) => {
                        out.insert(k.clone(), fv.clone());
                    }
                }
            }
            for (k, nv) in n {
                if f.contains_key(k) {
                    continue;
                }
                match t.get(k) {
                    Some(tv) if same_value(tv, nv) => {}
                    _ => {
                        out.insert(k.clone(), nv.clone());
                    }
                }
            }
            Value::Object(out)
        }
        (Value::Array(f), Value::Array(t), Value::Array(n)) if f.len() == t.len() => merge_array(f, t, n),
        _ => now.clone(),
    }
}

fn identity(v: &Value) -> (String, String) {
    (schema(v).to_string(), v.get("name").and_then(Value::as_str).unwrap_or("").to_string())
}

fn merge_array(f: &[Value], t: &[Value], n: &[Value]) -> Value {
    let same = lcs(t, n, same_value);
    let mut out = Vec::with_capacity(n.len());
    let (mut ti, mut nj) = (0usize, 0usize);
    let flush = |t_end: usize, n_end: usize, ti: usize, nj: usize, out: &mut Vec<Value>| {
        // Between two unchanged elements: pair what Film changed in place by kind and name, in order; the rest is new.
        let mut next_t = ti;
        for j in nj..n_end {
            match (next_t..t_end).find(|&i| identity(&t[i]) == identity(&n[j])) {
                Some(i) => {
                    out.push(merge3(&f[i], &t[i], &n[j]));
                    next_t = i + 1;
                }
                None => out.push(n[j].clone()),
            }
        }
    };
    for &(i, j) in &same {
        flush(i, j, ti, nj, &mut out);
        out.push(f[i].clone());
        ti = i + 1;
        nj = j + 1;
    }
    flush(t.len(), n.len(), ti, nj, &mut out);
    Value::Array(out)
}

// ---------------------------------------------------------------------------------------------
// Open and save.
// ---------------------------------------------------------------------------------------------

/// Film's rendering of `p`: Film's own part as Film writes it, Cut's parts in Cut's shape in their places.
fn render(p: &Project, s: &Shared) -> Result<(Value, super::cut::Rendered), SharedError> {
    let stripped = s.cut.strip(p, &s.film_items);
    let mut report = Report::default();
    let text = export_project(&stripped, &ExportOptions::default(), &mut report).map_err(|e| SharedError::Unreadable(e.to_string()))?;
    let mut doc: Value = serde_json::from_str(&text).map_err(|e| SharedError::Unreadable(e.to_string()))?;
    let film_seqs = sequences_of(&stripped);
    let made = s.cut.place(p, &film_seqs, &mut doc).map_err(SharedError::NotRoundTrip)?;
    Ok((doc, made))
}

/// Open the shared file: Film's project with Cut's parts as Film's own types, the first sequence to show, and what Film holds open.
pub fn open_shared(name: &str, bytes: &[u8]) -> Result<(Project, Option<ItemId>, Shared), SharedError> {
    let file: Value = serde_json::from_slice(bytes).map_err(|e| SharedError::Unreadable(e.to_string()))?;
    let (film, split) = split(&file);
    let stem = file.get("name").and_then(Value::as_str).unwrap_or(name).to_string();
    let (mut p, reconciled) = film_part(&film, &stem)?;
    for item in p.items.values() {
        if let ItemKind::Media(m) = &item.kind
            && let MediaRef::File { path } = &m.media
            && !is_oneos_media(path)
        {
            return Err(SharedError::MediaNotReferenced(item.name.clone()));
        }
    }
    let film_items: BTreeSet<ItemId> = p.items.keys().copied().collect();
    let film_seqs = sequences_of(&p);
    let cut = CutParts::read(&mut p, &film_seqs, split, &file).map_err(SharedError::NotRoundTrip)?;
    let mut s = Shared { file: file.clone(), then: Value::Null, cut, film_items };
    s.then = render(&p, &s)?.0;
    if let Some(at) = first_damage(&file, &s.then, reconciled) {
        return Err(SharedError::NotRoundTrip(format!("would be written back differently at {at}")));
    }
    let first = film_seqs.first().copied().or_else(|| s.cut.first_sequence());
    Ok((p, first, s))
}

/// The file to write for `p`, refused by name when a part cannot be written in its editor's shape; `s` then holds it as the file.
pub fn save_shared(p: &Project, s: &mut Shared) -> Result<Vec<u8>, SharedError> {
    let (now, made) = render(p, s)?;
    let out = merge3(&s.file, &s.then, &now);
    let bytes = serde_json::to_vec_pretty(&out).map_err(|e| SharedError::Unreadable(e.to_string()))?;
    s.cut.adopt(made);
    s.file = out;
    s.then = now;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge3_keeps_what_film_did_not_change_and_another_editors_keys() {
        let file = json!({"a": {"rate": 120000, "value": 240000}, "metadata": {"opencut": {"x": 1}, "filmcraft": {"k": 1, "source_in_ticks": 5}}, "n": "x"});
        let then = json!({"a": {"rate": 24, "value": 48}, "metadata": {"filmcraft": {"k": 1, "source_in_ticks": 7}}, "n": "x"});
        assert_eq!(merge3(&file, &then, &then), file, "an unedited save is the file");
        let now = json!({"a": {"rate": 24, "value": 48}, "metadata": {"filmcraft": {"k": 2, "source_in_ticks": 7}}, "n": "y"});
        let out = merge3(&file, &then, &now);
        assert_eq!(out, json!({"a": {"rate": 120000, "value": 240000}, "metadata": {"opencut": {"x": 1}, "filmcraft": {"k": 2, "source_in_ticks": 5}}, "n": "y"}));
    }

    #[test]
    fn merge3_arrays_pair_changed_elements_and_keep_unchanged_ones() {
        let c = |n: &str, v: i64, extra: bool| {
            let mut x = json!({"OTIO_SCHEMA": "Clip.2", "name": n, "v": v, "metadata": {}});
            if extra {
                x["metadata"]["opencut"] = json!({"e": 1});
            }
            x
        };
        let file = json!([c("a", 1, true), c("b", 2, true), c("c", 3, false)]);
        let then = json!([c("a", 1, false), c("b", 2, false), c("c", 3, false)]);
        // b trimmed, a new clip after it, c removed
        let now = json!([c("a", 1, false), c("b", 9, false), c("d", 4, false)]);
        let out = merge3(&file, &then, &now);
        assert_eq!(out, json!([c("a", 1, true), {"OTIO_SCHEMA": "Clip.2", "name": "b", "v": 9, "metadata": {"opencut": {"e": 1}}}, c("d", 4, false)]));
    }

    #[test]
    fn the_gate_names_what_film_would_damage() {
        let file = json!({"metadata": {"opencut": {"x": 1}}, "t": {"OTIO_SCHEMA": "RationalTime.1", "rate": 120000, "value": 120000}, "keep": 1});
        let then = json!({"metadata": {"filmcraft": {}}, "t": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 24}, "keep": 1});
        assert_eq!(first_damage(&file, &then, false), None);
        let lost = json!({"metadata": {"filmcraft": {}}, "t": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 24}});
        assert_eq!(first_damage(&file, &lost, false), Some("/keep".into()));
        let late = json!({"metadata": {"filmcraft": {}}, "t": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 25}, "keep": 1});
        assert_eq!(first_damage(&file, &late, false), Some("/t".into()));
    }
}
