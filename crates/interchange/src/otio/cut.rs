//! Cut's parts of the shared video project as Film's own types, and back in Cut's shape (OneOS doctrine files/989; Mona 2026-11-09: "film
//! should be able to edit cut though").
//!
//! Cut (OpenCut) saves its parts as OpenTimelineIO with its own data under `metadata.opencut`: a scene is a `Timeline.1`
//! (`metadata.opencut.scene`), a track a `Track.1` (`metadata.opencut.track`; overlapping elements as a `Stack.1` of lanes), an element a
//! `Clip.2` placed by `Gap.1`s (`metadata.opencut.element`; a medium is an `ExternalReference.1` to a OneOS file, a text, sticker, graphic
//! or effect a `GeneratorReference.1` of kind `OpenCut.<type>`), a bookmark a `Marker.2` (`metadata.opencut.bookmark`). Times are Cut's
//! ticks (120000 per second) or a track's own rate.
//!
//! Each part opens in Film as Film's own type:
//!
//! | Cut | Film |
//! |---|---|
//! | scene | a sequence |
//! | track (video, image, text, graphic, effect, audio) | a track; stacked lanes are one track each, grouped as one of Cut's |
//! | hidden / muted | the track's output (eye) / mute |
//! | video, image, audio element | a clip of the OneOS medium |
//! | text element | a graphic clip with a text layer (content, font, size, colour, alignment, weight, style, decoration, tracking, background) |
//! | graphic element (rectangle, ellipse, polygon) | a graphic clip with a shape layer (fill, stroke, stroke width, corner radius, sides) |
//! | effect element (blur) | an adjustment layer with Gaussian Blur |
//! | transform, opacity, hidden | the clip's Motion, Opacity and output |
//! | an element's blur effects | Gaussian Blur on the clip |
//! | bookmark | a sequence marker (time, duration, note, colour) |
//!
//! A save writes each part back in Cut's shape: the part as the file held it, with only the fields Film changed rewritten (a field-level
//! three-way: Film's value now against Film's value when it was opened), so every field Film does not edit comes back exactly and Cut
//! reopens it editable. A part opens only when its fields read back from Film exactly as the file holds them; one Film would damage
//! stays LOCKED (shown, carried untouched), named with its reason. What Film makes on Cut's tracks (a text, a shape, a blur adjustment
//! layer, a clip of a OneOS medium, a marker) is written as Cut writes it; what has no place in Cut's shape is refused at save, by name.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use filmcraft_geom::Vec2;
use filmcraft_project::{
    ClipId, EffectInstance, HexColor, ItemId, ItemKind, Keyframe, Label, Marker, MarkerId, MarkerKind, MediaRef, Param, ParamValue, Project, Track, TrackId, TrackItem,
    TrackKind, find_effect, graphic,
};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Map, Value, json};

use super::cut_keys::{CutKey, cut_to_film, film_to_cut, same_curve};
use super::oneos::{Split, oneos_media};
use super::{read_tr, schema_name};
use crate::common::{empty_sequence, rate_from_f64, settings_for};
use crate::is_oneos_media;

/// Cut's clock: ticks per second.
pub const CUT_TICKS: i64 = 120_000;
const PER_CUT_TICK: i64 = TICKS_PER_SECOND / CUT_TICKS;
const ELEMENT: &str = "/metadata/opencut/element";

type Fields = BTreeMap<String, Value>;

/// What an element of Cut's is in Film.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Image,
    Audio,
    Text,
    Shape,
    Effect,
}

impl Kind {
    fn cut_type(self) -> &'static str {
        match self {
            Kind::Video => "video",
            Kind::Image => "image",
            Kind::Audio => "audio",
            Kind::Text => "text",
            Kind::Shape => "graphic",
            Kind::Effect => "effect",
        }
    }
    fn generated(self) -> bool {
        matches!(self, Kind::Text | Kind::Shape | Kind::Effect)
    }
}

/// A part of Cut's that Film shows but does not edit (it would damage it), carried untouched.
#[derive(Debug, Clone, PartialEq)]
pub enum Locked {
    /// A timeline of Cut's, before the Film sequence `after` (`None`: first).
    Timeline { after: Option<ItemId>, order: usize, value: Value, reason: String },
    /// A track of Cut's in the sequence `seq`, after the track `after` (`None`: first).
    Track { seq: ItemId, after: Option<TrackId>, order: usize, value: Value, reason: String },
}

impl Locked {
    pub fn named(&self) -> String {
        let name = |v: &Value| v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        match self {
            Locked::Timeline { value, reason, .. } => format!("the timeline \"{}\" (Cut's): {reason}", name(value)),
            Locked::Track { value, reason, .. } => format!("the track \"{}\" (Cut's): {reason}", name(value)),
        }
    }
}

#[derive(Debug, Clone)]
struct Scene {
    seq: ItemId,
    after: Option<ItemId>,
    order: usize,
    origin: Value,
    name0: String,
}

#[derive(Debug, Clone)]
struct CutTrack {
    seq: ItemId,
    lanes: Vec<TrackId>,
    kind: TrackKind,
    /// In a Film timeline: the Film track it follows. In a scene: unused (its place is `order`).
    after: Option<TrackId>,
    order: usize,
    origin: Value,
    f0: Fields,
    ty: String,
}

#[derive(Debug, Clone)]
struct CutClip {
    origin: Value,
    f0: Fields,
    /// Each of Cut's channels Film shows, as Film's keyframes read back into Cut's shape when the file was opened (or last saved).
    a0: Anims,
    /// Each of Cut's held channels (a text's font, content, alignment, weight, style, decoration) Film shows, as read back when opened.
    h0: Held,
    kind: Kind,
    time0: (Tick, Tick, Tick),
}

#[derive(Debug, Clone)]
struct Mark {
    origin: Value,
    f0: Fields,
    time0: Tick,
}

/// Cut's parts of the open file as Film entities, and what Film carries locked.
#[derive(Debug, Clone, Default)]
pub struct CutParts {
    /// The file is a Cut project (`metadata.opencut.project`): elements and tracks carry Cut's ids, the root its media list.
    project_mode: bool,
    /// The root's metadata other than Film's, as the file holds it.
    root: Map<String, Value>,
    /// The frame Cut draws on (width, height).
    frame: (u32, u32),
    scenes: Vec<Scene>,
    tracks: Vec<CutTrack>,
    clips: BTreeMap<ClipId, CutClip>,
    marks: BTreeMap<MarkerId, Mark>,
    pub locked: Vec<Locked>,
}

/// What a render made of each of Cut's parts (it becomes what the file holds once saved).
#[derive(Debug, Default)]
pub(super) struct Rendered {
    scenes: Vec<Scene>,
    tracks: Vec<CutTrack>,
    clips: BTreeMap<ClipId, CutClip>,
    marks: BTreeMap<MarkerId, Mark>,
    media: Vec<Value>,
}

// ---------------------------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------------------------

fn cut_ticks(t: Tick) -> Option<i64> {
    (t.0 % PER_CUT_TICK == 0).then_some(t.0 / PER_CUT_TICK)
}

fn rt(value: i64, rate: f64) -> Value {
    json!({"OTIO_SCHEMA": "RationalTime.1", "rate": if rate.fract() == 0.0 { json!(rate as i64) } else { json!(rate) }, "value": value})
}

/// `t` at `rate` when it is a whole number there.
fn whole_at(t: Tick, rate: f64) -> Option<i64> {
    let r = rate_from_f64(rate);
    let v = t.0 as i128 * r.num as i128;
    let d = TICKS_PER_SECOND as i128 * r.den as i128;
    (v % d == 0).then(|| (v / d) as i64)
}

/// A start and a duration at `rate` when both are whole there, else in Cut's exact ticks; refused when neither.
fn time_range(start: Tick, dur: Tick, rate: f64) -> Result<Value, String> {
    if let (Some(s), Some(d)) = (whole_at(start, rate), whole_at(dur, rate)) {
        return Ok(json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(s, rate), "duration": rt(d, rate)}));
    }
    match (cut_ticks(start), cut_ticks(dur)) {
        (Some(s), Some(d)) => Ok(json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": rt(s, CUT_TICKS as f64), "duration": rt(d, CUT_TICKS as f64)})),
        _ => Err("time-not-in-cut: a time Film set is not a whole tick of Cut's clock (120000 per second)".into()),
    }
}

fn round9(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let s = 1e9;
    (x * s).round() / s
}

fn num(x: f64) -> Value {
    let x = round9(x);
    if x.fract() == 0.0 && x.abs() < 9e15 { json!(x as i64) } else { json!(x) }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(f64::NAN), y.as_f64().unwrap_or(f64::NAN));
            x == y || (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0)
        }
        (Value::String(x), Value::String(y)) if x.starts_with('#') && y.starts_with('#') => x.eq_ignore_ascii_case(y),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| same(p, q))),
        _ => a == b,
    }
}

/// `#rrggbb` or `#rrggbbaa` as straight sRGB floats.
fn color_of(v: &Value) -> Result<[f32; 4], String> {
    let s = v.as_str().ok_or("a colour that is not text")?;
    let h = s.strip_prefix('#').ok_or("a colour that is not #rrggbb")?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("zz"), 16).map(|b| b as f32 / 255.0);
    match h.len() {
        6 => Ok([byte(0).map_err(|e| e.to_string())?, byte(2).map_err(|e| e.to_string())?, byte(4).map_err(|e| e.to_string())?, 1.0]),
        8 => Ok([byte(0).map_err(|e| e.to_string())?, byte(2).map_err(|e| e.to_string())?, byte(4).map_err(|e| e.to_string())?, byte(6).map_err(|e| e.to_string())?]),
        _ => Err(format!("the colour {s} is not #rrggbb or #rrggbbaa")),
    }
}

fn hex_of(c: [f32; 4]) -> Value {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    if b(c[3]) == 255 { json!(format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))) } else { json!(format!("#{:02x}{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]), b(c[3]))) }
}

/// Set `ptr` (a JSON pointer) in `v`, making objects on the way; `Null` removes the key (Cut leaves an unset optional field out).
fn put(v: &mut Value, ptr: &str, x: Value) {
    let parts: Vec<&str> = ptr.split('/').skip(1).collect();
    let mut cur = v;
    for (i, k) in parts.iter().enumerate() {
        if !cur.is_object() {
            *cur = json!({});
        }
        let Value::Object(m) = cur else { return };
        if i + 1 == parts.len() {
            if x.is_null() {
                m.remove(*k);
            } else {
                m.insert((*k).to_string(), x);
            }
            return;
        }
        cur = m.entry((*k).to_string()).or_insert_with(|| json!({}));
    }
}

// ---------------------------------------------------------------------------------------------
// Film's side of a field
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Frame {
    w: f64,
    h: f64,
}

fn fx<'a>(ti: &'a TrackItem, id: &str) -> Option<&'a EffectInstance> {
    ti.effects.iter().find(|e| e.effect == id)
}

fn fx_mut<'a>(ti: &'a mut TrackItem, id: &str) -> Result<&'a mut EffectInstance, String> {
    ti.effects.iter_mut().find(|e| e.effect == id).ok_or_else(|| format!("the clip has no {id}"))
}

fn layer(ti: &TrackItem) -> Option<&EffectInstance> {
    ti.effects.iter().find(|e| graphic::is_layer(e))
}

fn layer_mut(ti: &mut TrackItem) -> Result<&mut EffectInstance, String> {
    ti.effects.iter_mut().find(|e| graphic::is_layer(e)).ok_or_else(|| "the clip has no graphic layer".to_string())
}

/// A parameter's value (when it is keyframed, its value without them: Cut's static field; the keyframes are Cut's channel, `anims`).
fn param<'a>(e: &'a EffectInstance, id: &str) -> Result<&'a ParamValue, String> {
    Ok(&e.params.get(id).ok_or_else(|| format!("{} has no {id}", e.effect))?.value)
}

fn set(e: &mut EffectInstance, id: &str, v: ParamValue) {
    e.params.insert(id.to_string(), Param::new(v));
}

fn float(e: &EffectInstance, id: &str) -> Result<f64, String> {
    match param(e, id)? {
        ParamValue::Float(x) => Ok(*x),
        _ => Err(format!("{}'s {id} is not a number", e.effect)),
    }
}

fn boolean(e: &EffectInstance, id: &str) -> Result<bool, String> {
    match param(e, id)? {
        ParamValue::Bool(x) => Ok(*x),
        _ => Err(format!("{}'s {id} is not a switch", e.effect)),
    }
}

fn want_f64(v: &Value) -> Result<f64, String> {
    v.as_f64().ok_or_else(|| format!("{v} is not a number"))
}

fn want_str(v: &Value) -> Result<&str, String> {
    v.as_str().ok_or_else(|| format!("{v} is not text"))
}

type Get = Box<dyn Fn(Frame, &TrackItem, &Value) -> Result<Value, String>>;
type Set = Box<dyn Fn(Frame, &mut TrackItem, &Value) -> Result<(), String>>;

/// One field of an element: its place in Cut's clip, its value when Cut leaves it out, and Film's side of it.
struct Field {
    ptr: String,
    default: Value,
    get: Get,
    set: Set,
}

fn field(ptr: &str, default: Value, get: impl Fn(Frame, &TrackItem, &Value) -> Result<Value, String> + 'static, set: impl Fn(Frame, &mut TrackItem, &Value) -> Result<(), String> + 'static) -> Field {
    Field { ptr: format!("{ELEMENT}{ptr}"), default, get: Box::new(get), set: Box::new(set) }
}

/// A parameter of an effect on the clip, as a number Cut scales by `k`.
fn scaled(ptr: &str, default: f64, effect: &'static str, id: &'static str, k: impl Fn(Frame) -> f64 + Copy + 'static) -> Field {
    field(ptr, json!(default), move |f, ti, _| Ok(num(float(fx(ti, effect).ok_or(format!("the clip has no {effect}"))?, id)? / k(f))), move |f, ti, v| {
        let x = want_f64(v)?;
        set(fx_mut(ti, effect)?, id, ParamValue::Float(x * k(f)));
        Ok(())
    })
}

/// A parameter of the clip's graphic layer.
fn on_layer(ptr: &str, default: Value, get: impl Fn(Frame, &EffectInstance) -> Result<Value, String> + 'static, put_: impl Fn(Frame, &mut EffectInstance, &Value) -> Result<(), String> + 'static) -> Field {
    field(ptr, default, move |f, ti, _| get(f, layer(ti).ok_or("the clip has no graphic layer")?), move |f, ti, v| put_(f, layer_mut(ti)?, v))
}

fn layer_color(ptr: &str, default: &str, id: &'static str, switch: Option<&'static str>) -> Field {
    on_layer(
        ptr,
        json!(default),
        move |_, e| match param(e, id)? {
            ParamValue::Color(c) => Ok(hex_of(*c)),
            _ => Err(format!("{id} is not a colour")),
        },
        move |_, e, v| {
            set(e, id, ParamValue::Color(color_of(v)?));
            if let Some(s) = switch {
                set(e, s, ParamValue::Bool(true));
            }
            Ok(())
        },
    )
}

fn layer_float(ptr: &str, default: f64, id: &'static str, k: impl Fn(Frame) -> f64 + Copy + 'static) -> Field {
    on_layer(ptr, json!(default), move |f, e| Ok(num(float(e, id)? / k(f))), move |f, e, v| {
        set(e, id, ParamValue::Float(want_f64(v)? * k(f)));
        Ok(())
    })
}

fn layer_text(ptr: &str, default: &str, id: &'static str) -> Field {
    on_layer(
        ptr,
        json!(default),
        move |_, e| match param(e, id)? {
            ParamValue::Text(t) => Ok(json!(t)),
            _ => Err(format!("{id} is not text")),
        },
        move |_, e, v| {
            set(e, id, ParamValue::Text(want_str(v)?.to_string()));
            Ok(())
        },
    )
}

/// A choice of Cut's (text) as a switch of Film's.
fn layer_switch(ptr: &str, default: &'static str, on: &'static str, id: &'static str) -> Field {
    on_layer(ptr, json!(default), move |_, e| Ok(json!(if boolean(e, id)? { on } else { default })), move |_, e, v| {
        let s = want_str(v)?;
        if s != on && s != default {
            return Err(format!("{s} is not {default} or {on}"));
        }
        set(e, id, ParamValue::Bool(s == on));
        Ok(())
    })
}

/// The fields of the clip's transform and opacity (Motion and Opacity in Film; position is from the frame's centre in Cut).
fn look_fields(origin: &Value) -> Vec<Field> {
    let tf = origin.pointer(&format!("{ELEMENT}/transform"));
    let uniform_only = tf.is_some_and(|t| t.get("scale").is_some());
    let mut v = vec![
        field("/hidden", json!(false), |_, ti, _| Ok(json!(!ti.enabled)), |_, ti, v| {
            ti.enabled = !v.as_bool().ok_or("hidden is not a switch")?;
            Ok(())
        }),
        scaled("/opacity", 1.0, "opacity", "opacity", |_| 100.0),
        field("/transform/rotate", json!(0), |_, ti, _| Ok(num(float(fx(ti, "motion").ok_or("the clip has no motion")?, "rotation")?)), |_, ti, v| {
            set(fx_mut(ti, "motion")?, "rotation", ParamValue::Float(want_f64(v)?));
            Ok(())
        }),
    ];
    for (axis, i) in [("x", 0usize), ("y", 1usize)] {
        v.push(field(
            &format!("/transform/position/{axis}"),
            json!(0),
            move |f, ti, _| match param(fx(ti, "motion").ok_or("the clip has no motion")?, "position")? {
                ParamValue::Vec2(p) => {
                    let (c, x) = if i == 0 { (f.w / 2.0, p.x) } else { (f.h / 2.0, p.y) };
                    Ok(num(if x.is_nan() { 0.0 } else { x - c }))
                }
                _ => Err("motion's position is not a point".into()),
            },
            move |f, ti, v| {
                let d = want_f64(v)?;
                let m = fx_mut(ti, "motion")?;
                let mut p = match m.params.get("position").map(|p| &p.value) {
                    Some(ParamValue::Vec2(p)) if !p.x.is_nan() && !p.y.is_nan() => *p,
                    _ => Vec2::new(f.w / 2.0, f.h / 2.0),
                };
                if i == 0 {
                    p.x = f.w / 2.0 + d;
                } else {
                    p.y = f.h / 2.0 + d;
                }
                // The frame's centre is Film's automatic position.
                let p = if p.x == f.w / 2.0 && p.y == f.h / 2.0 { Vec2::new(f64::NAN, f64::NAN) } else { p };
                set(m, "position", ParamValue::Vec2(p));
                Ok(())
            },
        ));
    }
    if uniform_only {
        v.push(field(
            "/transform/scale",
            json!(1),
            |_, ti, _| {
                let m = fx(ti, "motion").ok_or("the clip has no motion")?;
                if !boolean(m, "uniform_scale")? {
                    return Err("scaled unevenly in Film, which this element's single scale does not hold".into());
                }
                Ok(num(float(m, "scale")? / 100.0))
            },
            |_, ti, v| {
                let m = fx_mut(ti, "motion")?;
                set(m, "scale", ParamValue::Float(want_f64(v)? * 100.0));
                set(m, "uniform_scale", ParamValue::Bool(true));
                Ok(())
            },
        ));
    } else {
        v.push(field(
            "/transform/scaleX",
            json!(1),
            |_, ti, _| {
                let m = fx(ti, "motion").ok_or("the clip has no motion")?;
                Ok(num(float(m, if boolean(m, "uniform_scale")? { "scale" } else { "scale_width" })? / 100.0))
            },
            |_, ti, v| {
                let m = fx_mut(ti, "motion")?;
                set(m, "scale_width", ParamValue::Float(want_f64(v)? * 100.0));
                set(m, "uniform_scale", ParamValue::Bool(false));
                Ok(())
            },
        ));
        v.push(field(
            "/transform/scaleY",
            json!(1),
            |_, ti, _| Ok(num(float(fx(ti, "motion").ok_or("the clip has no motion")?, "scale")? / 100.0)),
            |_, ti, v| {
                let m = fx_mut(ti, "motion")?;
                let y = want_f64(v)? * 100.0;
                set(m, "scale", ParamValue::Float(y));
                let w = float(m, "scale_width").unwrap_or(y);
                set(m, "uniform_scale", ParamValue::Bool(w == y));
                Ok(())
            },
        ));
    }
    v
}

/// Cut's blur `intensity` (0..100, a Gaussian sigma of intensity/5 at 1920 wide) as Film's Gaussian Blur `blurriness` (sigma blurriness/2).
fn blur_k(f: Frame) -> f64 {
    0.4 * f.w / 1920.0
}

/// The element's blur effects as Gaussian Blur on the clip (the other effects of Film's have no place in Cut's).
fn effects_field() -> Field {
    field(
        "/effects",
        json!([]),
        |f, ti, origin| {
            let was = origin.pointer(&format!("{ELEMENT}/effects")).and_then(Value::as_array).cloned().unwrap_or_default();
            let mut out = Vec::new();
            for e in ti.effects.iter().filter(|e| !matches!(e.effect.as_str(), "motion" | "opacity" | "time_remap" | "volume" | "channel_volume" | "panner") && !graphic::is_layer(e)) {
                if e.effect != "gaussian_blur" {
                    return Err(format!("effect-not-in-cut: Film's {} has no equivalent among Cut's effects (Cut has blur)", e.effect));
                }
                let i = out.len();
                let id = was.get(i).and_then(|w| w.get("id")).cloned().unwrap_or_else(|| json!(format!("film-fx-{}-{i}", ti.id.0)));
                let mut x = was.get(i).filter(|w| w.get("type").and_then(Value::as_str) == Some("blur")).cloned().unwrap_or_else(|| json!({"id": id, "type": "blur", "params": {}, "enabled": true}));
                x["id"] = id;
                x["enabled"] = json!(e.enabled);
                x["params"]["intensity"] = num(float(e, "blurriness")? / blur_k(f));
                out.push(x);
            }
            Ok(Value::Array(out))
        },
        |f, ti, v| {
            for e in v.as_array().ok_or("effects is not a list")? {
                let ty = e.get("type").and_then(Value::as_str).unwrap_or("");
                if ty != "blur" {
                    return Err(format!("the effect {ty}, which Film has no map for"));
                }
                let mut g = find_effect("gaussian_blur").ok_or("Film has no Gaussian Blur")?.instance();
                g.enabled = e.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                let intensity = e.pointer("/params/intensity").and_then(Value::as_f64).unwrap_or(15.0);
                set(&mut g, "blurriness", ParamValue::Float(intensity * blur_k(f)));
                ti.effects.push(g);
            }
            Ok(())
        },
    )
}

/// The masks an element holds, carried; Film's masks on it have no place in Cut's yet (refused by name at save).
fn masks_field() -> Field {
    field(
        "/masks",
        Value::Null,
        |_, ti, origin| {
            if ti.effects.iter().any(|e| !e.masks.is_empty()) {
                return Err("mask-not-in-cut: a mask drawn in Film has no place in Cut's element yet".into());
            }
            Ok(origin.pointer(&format!("{ELEMENT}/masks")).cloned().unwrap_or(Value::Null))
        },
        |_, _, _| Ok(()),
    )
}

fn text_fields() -> Vec<Field> {
    vec![
        layer_text("/content", "", "text"),
        layer_float("/fontSize", 15.0, "size", |f| f.h / 90.0),
        layer_text("/fontFamily", "Arial", "font"),
        layer_color("/color", "#ffffff", "fill_color", Some("fill")),
        on_layer(
            "/textAlign",
            json!("center"),
            |_, e| match param(e, "align")? {
                ParamValue::Choice(0) => Ok(json!("left")),
                ParamValue::Choice(1) => Ok(json!("center")),
                ParamValue::Choice(2) => Ok(json!("right")),
                _ => Err("align-not-in-cut: Film's justified text has no place in Cut's alignment".into()),
            },
            |_, e, v| {
                let c = match want_str(v)? {
                    "left" => 0,
                    "center" => 1,
                    "right" => 2,
                    s => return Err(format!("the alignment {s}")),
                };
                set(e, "align", ParamValue::Choice(c));
                Ok(())
            },
        ),
        layer_switch("/fontWeight", "normal", "bold", "faux_bold"),
        layer_switch("/fontStyle", "normal", "italic", "faux_italic"),
        on_layer(
            "/textDecoration",
            json!("none"),
            |_, e| match (boolean(e, "underline")?, boolean(e, "strikethrough")?) {
                (false, false) => Ok(json!("none")),
                (true, false) => Ok(json!("underline")),
                (false, true) => Ok(json!("line-through")),
                (true, true) => Err("decoration-not-in-cut: underlined AND struck through, where Cut holds one".into()),
            },
            |_, e, v| {
                let s = want_str(v)?;
                set(e, "underline", ParamValue::Bool(s == "underline"));
                set(e, "strikethrough", ParamValue::Bool(s == "line-through"));
                Ok(())
            },
        ),
        layer_float("/letterSpacing", 0.0, "tracking", |_| 1.0),
        on_layer("/background/enabled", json!(false), |_, e| Ok(json!(boolean(e, "background")?)), |_, e, v| {
            set(e, "background", ParamValue::Bool(v.as_bool().ok_or("background.enabled is not a switch")?));
            // Cut's background colour carries its own opacity.
            set(e, "background_opacity", ParamValue::Float(100.0));
            Ok(())
        }),
        layer_color("/background/color", "#000000", "background_color", None),
    ]
}

const SHAPES: &[&str] = &["rectangle", "ellipse", "polygon"];

fn shape_fields(definition: &str) -> Vec<Field> {
    let mut v = vec![
        layer_color("/params/fill", "#ffffff", "fill_color", Some("fill")),
        layer_color("/params/stroke", "#000000", "stroke_color", None),
        on_layer("/params/strokeWidth", json!(0), |_, e| Ok(num(if boolean(e, "stroke")? { float(e, "stroke_width")? } else { 0.0 })), |_, e, v| {
            let w = want_f64(v)?;
            set(e, "stroke_width", ParamValue::Float(w));
            set(e, "stroke", ParamValue::Bool(w > 0.0));
            Ok(())
        }),
    ];
    if definition == "rectangle" || definition == "polygon" {
        v.push(layer_float("/params/cornerRadius", 0.0, "corner_radius", |_| 1.0));
    }
    if definition == "polygon" {
        v.push(layer_float("/params/sides", 5.0, "sides", |_| 1.0));
    }
    v
}

/// Cut's volume range in decibels (OpenCut `timeline/audio-constants.ts`).
const CUT_VOLUME_DB: (f64, f64) = (-60.0, 20.0);

/// Cut's volume IS Film's clip gain: both are decibels (OpenCut `animation/property-registry.ts`: volume, -60 to 20 dB; default 0).
fn volume_field() -> Field {
    field("/volume", json!(0), |_, ti, _| {
        if !(CUT_VOLUME_DB.0..=CUT_VOLUME_DB.1).contains(&ti.gain_db) {
            return Err(format!("volume-not-in-cut: a gain of {} dB, where Cut's volume holds {} to {} dB", ti.gain_db, CUT_VOLUME_DB.0, CUT_VOLUME_DB.1));
        }
        Ok(num(ti.gain_db))
    }, |_, ti, v| {
        ti.gain_db = want_f64(v)?;
        Ok(())
    })
}

fn audio_fields() -> Vec<Field> {
    vec![volume_field(), field("/muted", json!(false), |_, ti, _| Ok(json!(!ti.enabled)), |_, ti, v| {
        ti.enabled = !v.as_bool().ok_or("muted is not a switch")?;
        Ok(())
    })]
}

fn fields_for(kind: Kind, origin: &Value) -> Vec<Field> {
    let mut v = Vec::new();
    match kind {
        Kind::Audio => v.extend(audio_fields()),
        Kind::Effect => v.push(scaled("/params/intensity", 15.0, "gaussian_blur", "blurriness", blur_k)),
        _ => {
            v.extend(look_fields(origin));
            if kind == Kind::Video {
                v.push(volume_field());
            }
            v.push(effects_field());
            if kind != Kind::Text {
                v.push(masks_field());
            }
        }
    }
    match kind {
        Kind::Text => v.extend(text_fields()),
        Kind::Shape => v.extend(shape_fields(origin.pointer(&format!("{ELEMENT}/definitionId")).and_then(Value::as_str).unwrap_or("rectangle"))),
        _ => {}
    }
    v
}

// ---------------------------------------------------------------------------------------------
// Keyframes: Cut's channels as Film's keyframes, and back (`cut_keys`)
// ---------------------------------------------------------------------------------------------

/// Cut's channels by path, each a list of components (a number has one, a colour four: r, g, b, a).
type Anims = BTreeMap<String, Vec<Vec<CutKey>>>;

/// Where one of Cut's animated values is in Film.
#[derive(Debug, Clone, Copy, PartialEq)]
enum At {
    /// The clip's first effect of this id (Motion, Opacity, Gaussian Blur).
    Effect(&'static str),
    /// The clip's n-th Gaussian Blur (Cut's n-th blur effect).
    Blur(usize),
    /// The clip's graphic layer.
    Layer,
}

/// One of Cut's animated paths and the parameter of Film's that holds it: Film's component `i` is `cut * k + off`; a colour's are sRGB,
/// and Cut's are sRGB too in an `srgb` channel (Mona 2026-11-09: "Add sRGB channel to Cut"), linear light in Cut's own, which Film's keys
/// then ease in linear light (Mona: "Film eases linear too").
#[derive(Debug, Clone)]
struct Anim {
    path: String,
    at: At,
    param: &'static str,
    keys: &'static [&'static str],
    k: f64,
    off: f64,
    colour: Option<Space>,
}

/// Where a colour channel of Cut's keeps and eases its components.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Space {
    /// Cut's own channels: linear light.
    Linear,
    /// The space Film keys colours in, which Cut now holds as a channel of its own (`colorSpace: "srgb"`).
    Srgb,
}

impl Anim {
    /// Film's units in one of Cut's for each component (a sloped handle's rise is converted by it). A colour's rise is in its channel's own
    /// space, the space Film eases it in (Film's keys of a linear-light channel ease in linear light).
    fn per_cut(&self) -> Vec<f64> {
        match self.colour {
            Some(_) => vec![1.0; 4],
            None => vec![self.k],
        }
    }
}

const VALUE: &[&str] = &["value"];
const RGBA: &[&str] = &["r", "g", "b", "a"];

fn anim(path: &str, at: At, param: &'static str, k: f64, off: f64) -> Anim {
    Anim { path: path.into(), at, param, keys: VALUE, k, off, colour: None }
}
/// A colour of Cut's in the space its channel in the file keeps (`origin`); a channel the file does not have is Film's to write, in sRGB.
fn colour(path: &str, param: &'static str, origin: &Value) -> Anim {
    let space = match origin.pointer(&format!("{ELEMENT}/animations/bindings")).and_then(|b| b.get(path)).map(|b| b.get("colorSpace").and_then(Value::as_str)) {
        Some(Some("srgb")) | None => Space::Srgb,
        Some(_) => Space::Linear,
    };
    Anim { path: path.into(), at: At::Layer, param, keys: RGBA, k: 1.0, off: 0.0, colour: Some(space) }
}

/// Cut's animated paths of an element of `kind` and where Film holds each (`origin`: the clip as the file holds it, for its effects' ids).
fn anim_maps(kind: Kind, f: Frame, origin: &Value, gain: f64) -> Vec<Anim> {
    let mut v = Vec::new();
    match kind {
        Kind::Audio => v.push(Anim { at: At::Effect("volume"), ..anim("volume", At::Effect("volume"), "level", 1.0, -gain) }),
        Kind::Effect => v.push(anim("params.intensity", At::Effect("gaussian_blur"), "blurriness", blur_k(f), 0.0)),
        _ => {
            v.push(anim("opacity", At::Effect("opacity"), "opacity", 100.0, 0.0));
            v.push(anim("transform.rotate", At::Effect("motion"), "rotation", 1.0, 0.0));
            v.push(anim("transform.positionX", At::Effect("motion"), "position", 1.0, f.w / 2.0));
            v.push(anim("transform.positionY", At::Effect("motion"), "position", 1.0, f.h / 2.0));
            v.push(anim("transform.scaleX", At::Effect("motion"), "scale_width", 100.0, 0.0));
            v.push(anim("transform.scaleY", At::Effect("motion"), "scale", 100.0, 0.0));
            let effects = origin.pointer(&format!("{ELEMENT}/effects")).and_then(Value::as_array).cloned().unwrap_or_default();
            for (i, e) in effects.iter().filter(|e| e.get("type").and_then(Value::as_str) == Some("blur")).enumerate() {
                if let Some(id) = e.get("id").and_then(Value::as_str) {
                    v.push(anim(&format!("effects.{id}.params.intensity"), At::Blur(i), "blurriness", blur_k(f), 0.0));
                }
            }
        }
    }
    match kind {
        Kind::Text => {
            v.push(colour("color", "fill_color", origin));
            v.push(colour("background.color", "background_color", origin));
            // Mona 2026-11-09: "Build text animation in Cut": a text's size and tracking are channels of Cut's (its font, content, alignment,
            // weight, style and decoration are held channels, `held_maps`).
            v.push(anim("fontSize", At::Layer, "size", f.h / 90.0, 0.0));
            v.push(anim("letterSpacing", At::Layer, "tracking", 1.0, 0.0));
        }
        Kind::Shape => {
            v.push(colour("params.fill", "fill_color", origin));
            v.push(colour("params.stroke", "stroke_color", origin));
            v.push(anim("params.strokeWidth", At::Layer, "stroke_width", 1.0, 0.0));
            v.push(anim("params.cornerRadius", At::Layer, "corner_radius", 1.0, 0.0));
            v.push(anim("params.sides", At::Layer, "sides", 1.0, 0.0));
        }
        _ => {}
    }
    v
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}
fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn effect_at(ti: &TrackItem, at: At) -> Option<&EffectInstance> {
    match at {
        At::Effect(id) => fx(ti, id),
        At::Blur(n) => ti.effects.iter().filter(|e| e.effect == "gaussian_blur").nth(n),
        At::Layer => layer(ti),
    }
}
fn effect_at_mut(ti: &mut TrackItem, at: At) -> Option<&mut EffectInstance> {
    match at {
        At::Effect(id) => ti.effects.iter_mut().find(|e| e.effect == id),
        At::Blur(n) => ti.effects.iter_mut().filter(|e| e.effect == "gaussian_blur").nth(n),
        At::Layer => ti.effects.iter_mut().find(|e| graphic::is_layer(e)),
    }
}

/// A Film parameter's value as Cut's components (in Cut's units), in the order of `comps` (position: x then y; a colour: r, g, b, a).
fn film_parts(v: &ParamValue, comps: &[&Anim]) -> Vec<f64> {
    let raw: Vec<f64> = match v {
        ParamValue::Vec2(p) => vec![p.x, p.y],
        ParamValue::Color(c) => c.iter().map(|x| *x as f64).collect(),
        other => vec![other.as_f64().unwrap_or(0.0)],
    };
    match comps.first().and_then(|a| a.colour) {
        Some(Space::Linear) => return raw.iter().enumerate().map(|(i, x)| if i < 3 { srgb_to_linear(*x) } else { *x }).collect(),
        Some(Space::Srgb) => return raw,
        None => {}
    }
    comps.iter().enumerate().map(|(i, a)| (raw.get(i).copied().unwrap_or(0.0) - a.off) / a.k).collect()
}

fn film_value(like: &ParamValue, c: &[f64], comps: &[&Anim]) -> ParamValue {
    if let Some(space) = comps.first().and_then(|a| a.colour) {
        let s = |i: usize| (if i < 3 && space == Space::Linear { linear_to_srgb(c[i]) } else { c[i] }) as f32;
        return ParamValue::Color([s(0), s(1), s(2), s(3)]);
    }
    let x: Vec<f64> = comps.iter().enumerate().map(|(i, a)| c[i] * a.k + a.off).collect();
    match like {
        ParamValue::Vec2(_) => ParamValue::Vec2(Vec2::new(x[0], x.get(1).copied().unwrap_or(0.0))),
        ParamValue::Choice(_) => ParamValue::Choice(x[0].round().max(0.0) as u32),
        _ => ParamValue::Float(x[0]),
    }
}

/// Cut's channel at `path` (one list of keys per component, in `keys` order), when the element animates it.
fn cut_channel(origin: &Value, path: &str, keys: &[&str]) -> Option<Vec<Vec<CutKey>>> {
    let b = origin.pointer(&format!("{ELEMENT}/animations/bindings"))?.get(path)?;
    let comps = b.get("components")?.as_array()?;
    let mut out = Vec::new();
    for k in keys {
        let id = comps.iter().find(|c| c.get("key").and_then(Value::as_str) == Some(k))?.get("channelId")?.as_str()?;
        let ch = origin.pointer(&format!("{ELEMENT}/animations/channels"))?.get(id)?;
        out.push(ch.get("keys")?.as_array()?.iter().map(CutKey::read).collect::<Option<Vec<_>>>()?);
    }
    out.iter().any(|c| !c.is_empty()).then_some(out)
}

/// Write Cut's channel at `path` into the clip `c` (Cut's shape), or remove it when it holds no keys.
fn put_channel(c: &mut Value, path: &str, a: &Anim, comps: Option<&[Vec<CutKey>]>) {
    let bindings = format!("{ELEMENT}/animations/bindings");
    let was: Vec<String> = c.pointer(&bindings).and_then(|b| b.get(path)).and_then(|b| b.get("components")).and_then(Value::as_array).map(|x| x.iter().filter_map(|y| y.get("channelId").and_then(Value::as_str).map(str::to_string)).collect()).unwrap_or_default();
    match comps.filter(|x| x.iter().any(|k| !k.is_empty())) {
        None => {
            if let Some(Value::Object(m)) = c.pointer_mut(&bindings) {
                m.remove(path);
            }
            if let Some(Value::Object(m)) = c.pointer_mut(&format!("{ELEMENT}/animations/channels")) {
                for id in was {
                    m.remove(&id);
                }
            }
        }
        Some(comps) => {
            let ids: Vec<String> = a.keys.iter().enumerate().map(|(i, k)| was.get(i).cloned().unwrap_or_else(|| format!("{path}:{k}"))).collect();
            let mut b = json!({"path": path, "kind": if a.colour.is_some() { "color" } else { "number" }, "components": a.keys.iter().zip(&ids).map(|(k, id)| json!({"key": k, "channelId": id})).collect::<Vec<_>>()});
            let at = format!("{bindings}/{}", path.replace('~', "~0").replace('/', "~1"));
            if let Some(space) = a.colour {
                b["colorSpace"] = json!(if space == Space::Srgb { "srgb" } else { "srgb-linear" });
            }
            if c.pointer(&bindings).and_then(|x| x.get(path)).is_none() {
                put(c, &at, b);
            }
            for (i, id) in ids.iter().enumerate() {
                let keys: Vec<Value> = comps[i].iter().map(CutKey::write).collect();
                put(c, &format!("{ELEMENT}/animations/channels/{}", id.replace('~', "~0").replace('/', "~1")), json!({"kind": "scalar", "keys": keys}));
            }
        }
    }
}

/// Group `maps` by the Film parameter that holds them (position's x and y are one; a colour's four components are one).
fn groups(maps: &[Anim]) -> Vec<Vec<&Anim>> {
    let mut out: Vec<Vec<&Anim>> = Vec::new();
    for a in maps {
        match out.iter_mut().find(|g| g[0].at == a.at && g[0].param == a.param) {
            Some(g) => g.push(a),
            None => out.push(vec![a]),
        }
    }
    out
}

/// Cut's channels of the element (`origin`) as Film's keyframes on `ti`, and what they read back as (the file's `a0`); a reason when Film
/// cannot hold one (the track is then locked, named).
fn read_anims(kind: Kind, f: Frame, ti: &mut TrackItem, origin: &Value) -> Result<Anims, String> {
    let maps = anim_maps(kind, f, origin, ti.gain_db);
    // Uniform scale: Cut's two axes animate as one only when they are the same curve.
    let sx = cut_channel(origin, "transform.scaleX", VALUE);
    let sy = cut_channel(origin, "transform.scaleY", VALUE);
    if (sx.is_some() || sy.is_some()) && !matches!((&sx, &sy), (Some(x), Some(y)) if same_curve(&x[0], &y[0]))
        && let Some(m) = ti.effects.iter_mut().find(|e| e.effect == "motion")
    {
        set(m, "uniform_scale", ParamValue::Bool(false));
    }
    let uniform = fx(ti, "motion").and_then(|m| m.params.get("uniform_scale")).is_some_and(|p| matches!(p.value, ParamValue::Bool(true)));
    let mut a0 = Anims::new();
    for g in groups(&maps) {
        let chans: Vec<Option<Vec<Vec<CutKey>>>> = g.iter().map(|a| cut_channel(origin, &a.path, a.keys)).collect();
        if chans.iter().all(Option::is_none) {
            continue;
        }
        // A uniform scale's width is its height: Cut's X channel, the same curve as Y, is held by Y's keyframes.
        if uniform && g[0].param == "scale_width" {
            a0.insert(g[0].path.clone(), chans[0].clone().unwrap_or_default());
            continue;
        }
        let e = effect_at(ti, g[0].at).ok_or_else(|| format!("animates {}, which has no {} in Film", g[0].path, g[0].param))?;
        let like = e.params.get(g[0].param).map(|p| p.value.clone()).ok_or_else(|| format!("animates {}, which has no {} in Film", g[0].path, g[0].param))?;
        let statics = film_parts(&like, &g);
        // Each component as Cut's channel; one Cut does not animate is held at its value through the others' keys.
        let flat: Vec<Vec<CutKey>> = if g[0].colour.is_some() { chans[0].clone().unwrap_or_default() } else { chans.iter().map(|c| c.as_ref().map(|x| x[0].clone()).unwrap_or_default()).collect() };
        let lead = flat.iter().find(|c| !c.is_empty()).cloned().unwrap_or_default();
        let comps: Vec<Vec<CutKey>> = flat
            .iter()
            .enumerate()
            .map(|(i, c)| if c.is_empty() { lead.iter().map(|k| CutKey { value: statics[i], right: k.right.map(|h| (h.0, 0.0)), left: k.left.map(|h| (h.0, 0.0)), ..k.clone() }).collect() } else { c.clone() })
            .collect();
        let per_cut: Vec<f64> = if g[0].colour.is_some() { g[0].per_cut() } else { g.iter().map(|a| a.k).collect() };
        let keys = cut_to_film(&comps, &|c| film_value(&like, c, &g), &[], &per_cut).map_err(|why| format!("animates {}: {why}", g[0].path))?;
        let back = film_to_cut(&keys, &|v| film_parts(v, &g), &comps, &g[0].path, &per_cut)?;
        let tol = |a: &[CutKey], b: &[CutKey]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.time == y.time && (x.value - y.value).abs() <= 1e-6 * x.value.abs().max(1.0)) && same_curve(&round6(a), &round6(b));
        for (i, c) in flat.iter().enumerate() {
            if !c.is_empty() && !(if g[0].colour.is_some() { tol(c, &back[i]) } else { same_curve(c, &back[i]) }) {
                return Err(format!("animates {} in a way that does not read back from Film exactly", g[0].path));
            }
        }
        // Cut's own colour channels ease in linear light, and so do Film's keys of them (Mona 2026-11-09: "Film eases linear too").
        let linear = g[0].colour == Some(Space::Linear);
        let keys: Vec<_> = keys.into_iter().map(|k| filmcraft_project::Keyframe { linear_light: linear, ..k }).collect();
        if let Some(p) = effect_at_mut(ti, g[0].at).and_then(|e| e.params.get_mut(g[0].param)) {
            p.keyframes = keys;
        }
        if g[0].colour.is_some() {
            a0.insert(g[0].path.clone(), back);
        } else {
            for (i, a) in g.iter().enumerate() {
                a0.insert(a.path.clone(), vec![back[i].clone()]);
            }
        }
    }
    if uniform && let Some(y) = a0.get("transform.scaleY").cloned() {
        a0.insert("transform.scaleX".into(), y);
    }
    Ok(a0)
}

fn round6(a: &[CutKey]) -> Vec<CutKey> {
    a.iter().map(|k| CutKey { value: (k.value * 1e6).round() / 1e6, ..k.clone() }).collect()
}

/// Film's keyframes on Cut's element as Cut's channels, written into `c` where they changed since `a0`; the channels now. A reason (by
/// name) when Cut has no place for one.
fn write_anims(kind: Kind, f: Frame, ti: &TrackItem, c: &mut Value, a0: &Anims, name: &str) -> Result<Anims, String> {
    let maps = anim_maps(kind, f, c, ti.gain_db);
    let uniform = fx(ti, "motion").and_then(|m| m.params.get("uniform_scale")).is_some_and(|p| matches!(p.value, ParamValue::Bool(true)));
    let mut held: BTreeSet<(String, &'static str)> = BTreeSet::new();
    for h in held_maps(kind) {
        if let Some(e) = layer(ti) {
            for p in h.params {
                held.insert((e.effect.clone(), p));
            }
        }
    }
    let mut now = Anims::new();
    for g in groups(&maps) {
        let Some(e) = effect_at(ti, g[0].at) else { continue };
        held.insert((e.effect.clone(), g[0].param));
        let Some(p) = e.params.get(g[0].param) else { continue };
        if g[0].param == "scale_width" && uniform {
            continue;
        }
        let prior: Vec<Vec<CutKey>> = if g[0].colour.is_some() { a0.get(&g[0].path).cloned().unwrap_or_default() } else { g.iter().map(|a| a0.get(&a.path).and_then(|x| x.first().cloned()).unwrap_or_default()).collect() };
        let per_cut: Vec<f64> = if g[0].colour.is_some() { g[0].per_cut() } else { g.iter().map(|a| a.k).collect() };
        let chans = film_to_cut(&p.keyframes, &|v| film_parts(v, &g), &prior, &format!("film-{}-{}", ti.id.0, g[0].path), &per_cut).map_err(|why| format!("keyframes-not-in-cut: Film's {} {} on \"{name}\": {why}", e.effect, g[0].param))?;
        if let Some(space) = g[0].colour {
            // A colour is written in its channel's own space; Film eases it there too (its keys say so), so both draw it alike.
            let linear = space == Space::Linear;
            if p.keyframes.iter().any(|k| k.linear_light != linear) {
                return Err(format!("keyframes-not-in-cut: Film eases \"{name}\"'s {} in {}, where its channel in Cut eases in {}", g[0].path, if linear { "sRGB" } else { "linear light" }, if linear { "linear light" } else { "sRGB" }));
            }
            now.insert(g[0].path.clone(), chans);
        } else {
            for (i, a) in g.iter().enumerate() {
                now.insert(a.path.clone(), chans.get(i).cloned().map(|x| vec![x]).unwrap_or_default());
            }
        }
    }
    if uniform && let Some(y) = now.get("transform.scaleY").cloned() {
        now.insert("transform.scaleX".into(), y);
        held.insert(("motion".into(), "scale_width"));
    }
    // A parameter Film keyframes that Cut has no channel for is refused by name, never dropped.
    for e in &ti.effects {
        for (id, p) in &e.params {
            if !p.keyframes.is_empty() && !held.iter().any(|(x, y)| *x == e.effect && y == id) {
                return Err(format!("keyframes-not-in-cut: Film keyframes {}'s {id} on \"{name}\", which Cut does not animate", e.effect));
            }
        }
    }
    for a in &maps {
        let (Some(was), Some(is)) = (a0.get(&a.path).map(Vec::as_slice), now.get(&a.path).map(Vec::as_slice)) else {
            if let Some(is) = now.get(&a.path).filter(|x| x.iter().any(|k| !k.is_empty())) {
                put_channel(c, &a.path, a, Some(is));
            } else if a0.get(&a.path).is_some_and(|x| x.iter().any(|k| !k.is_empty())) {
                put_channel(c, &a.path, a, None);
            }
            continue;
        };
        let changed = was.len() != is.len() || was.iter().zip(is).any(|(x, y)| !same_curve(x, y));
        if changed {
            put_channel(c, &a.path, a, Some(is));
        }
    }
    Ok(now.into_iter().filter(|(_, v)| v.iter().any(|k| !k.is_empty())).collect())
}

/// Cut's held channels by path: each key's time (Cut's ticks) and value.
type Held = BTreeMap<String, Vec<(i64, String)>>;

/// One of a text's held values (Mona 2026-11-09: "Build text animation in Cut"): Cut's discrete channel at `path` and the parameters of
/// Film's text layer that hold it (both hold a value from key to key).
struct HeldMap {
    path: &'static str,
    params: &'static [&'static str],
    to_film: fn(&str) -> Result<Vec<ParamValue>, String>,
    from_film: fn(&[ParamValue]) -> Result<String, String>,
}

fn text_of_value(v: &[ParamValue]) -> Result<String, String> {
    match v.first() {
        Some(ParamValue::Text(t)) => Ok(t.clone()),
        other => Err(format!("{other:?} is not text")),
    }
}
fn switch_of(v: &[ParamValue], off: &str, on: &str) -> Result<String, String> {
    match v.first() {
        Some(ParamValue::Bool(b)) => Ok(if *b { on } else { off }.to_string()),
        other => Err(format!("{other:?} is not a switch")),
    }
}

fn held_maps(kind: Kind) -> Vec<HeldMap> {
    if kind != Kind::Text {
        return Vec::new();
    }
    vec![
        HeldMap { path: "content", params: &["text"], to_film: |s| Ok(vec![ParamValue::Text(s.into())]), from_film: text_of_value },
        HeldMap { path: "fontFamily", params: &["font"], to_film: |s| Ok(vec![ParamValue::Text(s.into())]), from_film: text_of_value },
        HeldMap {
            path: "textAlign",
            params: &["align"],
            to_film: |s| match s {
                "left" => Ok(vec![ParamValue::Choice(0)]),
                "center" => Ok(vec![ParamValue::Choice(1)]),
                "right" => Ok(vec![ParamValue::Choice(2)]),
                o => Err(format!("the alignment {o}")),
            },
            from_film: |v| match v.first() {
                Some(ParamValue::Choice(0)) => Ok("left".into()),
                Some(ParamValue::Choice(1)) => Ok("center".into()),
                Some(ParamValue::Choice(2)) => Ok("right".into()),
                _ => Err("align-not-in-cut: Film's justified text has no place in Cut's alignment".into()),
            },
        },
        HeldMap { path: "fontWeight", params: &["faux_bold"], to_film: |s| Ok(vec![ParamValue::Bool(s == "bold")]), from_film: |v| switch_of(v, "normal", "bold") },
        HeldMap { path: "fontStyle", params: &["faux_italic"], to_film: |s| Ok(vec![ParamValue::Bool(s == "italic")]), from_film: |v| switch_of(v, "normal", "italic") },
        HeldMap {
            path: "textDecoration",
            params: &["underline", "strikethrough"],
            to_film: |s| Ok(vec![ParamValue::Bool(s == "underline"), ParamValue::Bool(s == "line-through")]),
            from_film: |v| match (v.first(), v.get(1)) {
                (Some(ParamValue::Bool(false)), Some(ParamValue::Bool(false))) => Ok("none".into()),
                (Some(ParamValue::Bool(true)), Some(ParamValue::Bool(false))) => Ok("underline".into()),
                (Some(ParamValue::Bool(false)), Some(ParamValue::Bool(true))) => Ok("line-through".into()),
                _ => Err("decoration-not-in-cut: underlined AND struck through, where Cut holds one".into()),
            },
        },
    ]
}

/// Cut's held channel at `path`, when the element keys it.
fn held_channel(origin: &Value, path: &str) -> Option<Vec<(i64, String, Value)>> {
    let b = origin.pointer(&format!("{ELEMENT}/animations/bindings"))?.get(path)?;
    let id = b.get("components")?.as_array()?.first()?.get("channelId")?.as_str()?;
    let ch = origin.pointer(&format!("{ELEMENT}/animations/channels"))?.get(id)?;
    let keys: Vec<(i64, String, Value)> = ch.get("keys")?.as_array()?.iter().map(|k| Some((k.get("time")?.as_i64()?, k.get("value")?.as_str()?.to_string(), k.get("id").cloned().unwrap_or(Value::Null)))).collect::<Option<_>>()?;
    (!keys.is_empty()).then_some(keys)
}

/// Cut's held channels of a text as Film's keyframes on its text layer, and what they read back as. Before its first key a held channel of
/// Cut's shows the element's own value, where Film shows its first key's: so a channel whose first key is after the start is keyed at the
/// clip's start with the element's own value, which Film writes back only where Cut keyed it.
fn read_held(kind: Kind, ti: &mut TrackItem, origin: &Value) -> Result<Held, String> {
    let mut h0 = Held::new();
    for h in held_maps(kind) {
        let Some(keys) = held_channel(origin, h.path) else { continue };
        let own = origin.pointer(&format!("{ELEMENT}/{}", h.path)).and_then(Value::as_str).ok_or_else(|| format!("animates {} with no value of its own", h.path))?.to_string();
        let mut film: Vec<(i64, String)> = keys.iter().map(|(t, v, _)| (*t, v.clone())).collect();
        if film.first().is_some_and(|(t, _)| *t > 0) {
            film.insert(0, (0, own));
        }
        let e = layer_mut(ti).map_err(|e| format!("animates {}: {e}", h.path))?;
        for (i, p) in h.params.iter().enumerate() {
            let param = e.params.get_mut(*p).ok_or_else(|| format!("animates {}, which has no {p} in Film", h.path))?;
            param.keyframes = film
                .iter()
                .map(|(t, v)| {
                    let mut k = Keyframe::new(Tick(t * PER_CUT_TICK), (h.to_film)(v)?[i].clone());
                    k.interp = filmcraft_project::Interpolation::Hold;
                    Ok(k)
                })
                .collect::<Result<_, String>>()
                .map_err(|e: String| format!("animates {}: {e}", h.path))?;
        }
        h0.insert(h.path.to_string(), keys.into_iter().map(|(t, v, _)| (t, v)).collect());
    }
    Ok(h0)
}

/// Film's keyframes on a text's layer as Cut's held channels, written into `c` where they changed since `h0`; the channels now.
fn write_held(kind: Kind, ti: &TrackItem, c: &mut Value, h0: &Held, name: &str) -> Result<Held, String> {
    let mut now = Held::new();
    let Some(e) = layer(ti) else { return Ok(now) };
    for h in held_maps(kind) {
        let params: Vec<&Param> = h.params.iter().map(|p| e.params.get(*p).ok_or_else(|| format!("\"{name}\" has no {p} on its text layer"))).collect::<Result<_, _>>()?;
        let mut times: Vec<Tick> = params.iter().flat_map(|p| p.keyframes.iter().map(|k| k.time)).collect();
        times.sort();
        times.dedup();
        let mut keys: Vec<(i64, String)> = Vec::new();
        for t in &times {
            let at = cut_ticks(*t).ok_or_else(|| format!("keyframes-not-in-cut: Film keys \"{name}\"'s {} at Film tick {}, not a whole tick of Cut's clock", h.path, t.0))?;
            let v: Vec<ParamValue> = params.iter().map(|p| p.value_at(*t)).collect();
            keys.push((at, (h.from_film)(&v).map_err(|why| format!("keyframes-not-in-cut: \"{name}\"'s {}: {why}", h.path))?));
        }
        // The key at the start Film was given for Cut's own value is not Cut's key (unless Cut had one there).
        let own = (h.from_film)(&params.iter().map(|p| p.value.clone()).collect::<Vec<_>>()).map_err(|why| format!("\"{name}\"'s {}: {why}", h.path))?;
        let was = h0.get(h.path);
        if keys.len() > 1 && keys[0].0 == 0 && keys[0].1 == own && !was.is_some_and(|w| w.first().is_some_and(|k| k.0 == 0)) {
            keys.remove(0);
        }
        if was.map(Vec::as_slice).unwrap_or(&[]) != keys.as_slice() {
            put_held(c, h.path, &keys);
        }
        if !keys.is_empty() {
            now.insert(h.path.to_string(), keys);
        }
    }
    Ok(now)
}

/// Write Cut's held channel at `path` into the clip `c` (Cut's shape: a `discrete` binding and channel), or remove it when it holds no keys.
/// A key at a time Cut had keeps its id.
fn put_held(c: &mut Value, path: &str, keys: &[(i64, String)]) {
    let bindings = format!("{ELEMENT}/animations/bindings");
    let channels = format!("{ELEMENT}/animations/channels");
    let was = held_channel(c, path).unwrap_or_default();
    let id = c.pointer(&bindings).and_then(|b| b.get(path)).and_then(|b| b.pointer("/components/0/channelId")).and_then(Value::as_str).map(str::to_string);
    let esc = |s: &str| s.replace('~', "~0").replace('/', "~1");
    if keys.is_empty() {
        if let Some(Value::Object(m)) = c.pointer_mut(&bindings) {
            m.remove(path);
        }
        if let (Some(id), Some(Value::Object(m))) = (id, c.pointer_mut(&channels)) {
            m.remove(&id);
        }
        return;
    }
    let id = id.unwrap_or_else(|| format!("{path}:value"));
    put(c, &format!("{bindings}/{}", esc(path)), json!({"path": path, "kind": "discrete", "components": [{"key": "value", "channelId": id}]}));
    let out: Vec<Value> = keys
        .iter()
        .enumerate()
        .map(|(i, (t, v))| {
            let kid = was.iter().find(|w| w.0 == *t).map(|w| w.2.clone()).filter(|x| !x.is_null()).unwrap_or_else(|| json!(format!("film-{path}-{i}")));
            json!({"id": kid, "time": t, "value": v})
        })
        .collect();
    put(c, &format!("{channels}/{}", esc(&id)), json!({"kind": "discrete", "keys": out}));
}

fn read_fields(kind: Kind, f: Frame, ti: &TrackItem, origin: &Value) -> Result<Fields, String> {
    fields_for(kind, origin).iter().map(|x| Ok((x.ptr.clone(), (x.get)(f, ti, origin)?))).collect()
}

// ---------------------------------------------------------------------------------------------
// Reading Cut's parts into Film
// ---------------------------------------------------------------------------------------------

/// A Cut time as Film ticks (exact: a Cut tick is a whole number of Film's).
fn read_time(v: &Value) -> Option<(Tick, Tick)> {
    read_tr(v)
}

fn graphic_source(p: &mut Project, w: u32, h: u32, rate: FrameRate) -> ItemId {
    if let Some((id, _)) = p.items.iter().find(|(id, i)| !p.source_graphics.contains_key(id) && matches!(i.kind, ItemKind::Graphic { width, height, rate: r } if width == w && height == h && r == rate)) {
        return *id;
    }
    p.add_item("Graphic", Label::Rose, ItemKind::Graphic { width: w, height: h, rate }, None)
}

fn kind_of_generator(c: &Value) -> Result<Kind, String> {
    let k = c.pointer("/media_references/DEFAULT_MEDIA/generator_kind").and_then(Value::as_str).unwrap_or("");
    match k {
        "OpenCut.text" => Ok(Kind::Text),
        "OpenCut.graphic" => {
            let d = c.pointer(&format!("{ELEMENT}/definitionId")).and_then(Value::as_str).unwrap_or("");
            if SHAPES.contains(&d) {
                Ok(Kind::Shape)
            } else {
                Err(format!("holds the graphic \"{d}\", which Film's shape layer has no shape for yet"))
            }
        }
        "OpenCut.effect" => {
            let t = c.pointer(&format!("{ELEMENT}/effectType")).and_then(Value::as_str).unwrap_or("");
            if t == "blur" { Ok(Kind::Effect) } else { Err(format!("holds the effect \"{t}\", which Film has no map for")) }
        }
        "OpenCut.sticker" => Err("holds a sticker, which Film shows once its sticker layer is built".into()),
        other => Err(format!("holds the generator \"{other}\", which is not one of Cut's")),
    }
}

/// One of Cut's clips as a Film clip on a track of `kind`, and what Film keeps of it.
fn read_clip(p: &mut Project, c: &Value, track_kind: TrackKind, f: Frame, rate: FrameRate, (start, src, dur): (Tick, Tick, Tick)) -> Result<(TrackItem, CutClip), String> {
    let name = c.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let r = c.pointer("/media_references/DEFAULT_MEDIA").ok_or_else(|| format!("holds the clip \"{name}\" without a medium"))?;
    let (w, h) = (f.w as u32, f.h as u32);
    let (kind, mut ti) = match schema_name(r) {
        "ExternalReference" => {
            let url = r.get("target_url").and_then(Value::as_str).unwrap_or("");
            if !is_oneos_media(url) {
                return Err(format!("holds \"{name}\", whose medium is not a OneOS file"));
            }
            let kind = if track_kind == TrackKind::Audio {
                Kind::Audio
            } else if c.pointer(&format!("{ELEMENT}/type")).and_then(Value::as_str) == Some("image") {
                Kind::Image
            } else {
                Kind::Video
            };
            let length = c.pointer(&format!("{ELEMENT}/sourceDuration")).and_then(Value::as_i64).map(|t| Tick(t * PER_CUT_TICK));
            let mname = r.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(&name).to_string();
            let item = oneos_media(p, url, &mname, track_kind, length);
            (kind, p.make_track_item(item, track_kind, start, TimeRange::new(src, dur), rate).ok_or_else(|| format!("could not place \"{name}\""))?)
        }
        "GeneratorReference" => {
            let kind = kind_of_generator(c)?;
            if track_kind != TrackKind::Video {
                return Err(format!("holds the {} \"{name}\" on an audio track", kind.cut_type()));
            }
            let item = match kind {
                Kind::Effect => p.add_item("Adjustment Layer", Label::Lavender, ItemKind::AdjustmentLayer { width: w, height: h, rate, duration: src + dur }, None),
                _ => graphic_source(p, w, h, rate),
            };
            let mut ti = p.make_track_item(item, TrackKind::Video, start, TimeRange::new(src, dur), rate).ok_or_else(|| format!("could not place \"{name}\""))?;
            let centre = Vec2::new(f.w / 2.0, f.h / 2.0);
            match kind {
                Kind::Text => {
                    let mut l = graphic::new_text_layer("", centre, 100.0);
                    set(&mut l, "align", ParamValue::Choice(1));
                    ti.effects.push(l);
                }
                Kind::Effect => {
                    let mut g = find_effect("gaussian_blur").ok_or("Film has no Gaussian Blur")?.instance();
                    g.enabled = true;
                    ti.effects.push(g);
                }
                Kind::Shape => {
                    let d = c.pointer(&format!("{ELEMENT}/definitionId")).and_then(Value::as_str).unwrap_or("rectangle");
                    let shape = SHAPES.iter().position(|s| *s == d).unwrap_or(0) as u32;
                    // Cut draws a graphic on a 512 px square, scaled by its transform.
                    let mut l = graphic::new_shape_layer(shape, centre, Vec2::new(512.0, 512.0), Vec::new());
                    set(&mut l, "fill", ParamValue::Bool(true));
                    ti.effects.push(l);
                }
                _ => {}
            }
            (kind, ti)
        }
        other => return Err(format!("holds \"{name}\", whose medium is a {other}")),
    };
    ti.name = name;
    ti.duration = dur;
    ti.source_in = src;
    for x in fields_for(kind, c) {
        let v = c.pointer(&x.ptr).cloned().filter(|v| !v.is_null()).unwrap_or_else(|| x.default.clone());
        if v.is_null() {
            continue;
        }
        (x.set)(f, &mut ti, &v).map_err(|e| format!("holds \"{}\" whose {} Film cannot show: {e}", ti.name, x.ptr.trim_start_matches(ELEMENT)))?;
    }
    let f0 = read_fields(kind, f, &ti, c).map_err(|e| format!("holds \"{}\": {e}", ti.name))?;
    for x in fields_for(kind, c) {
        let file = c.pointer(&x.ptr).cloned().unwrap_or_else(|| x.default.clone());
        let film = f0.get(&x.ptr).cloned().unwrap_or(Value::Null);
        if !same(&file, &film) {
            return Err(format!("holds \"{}\" whose {} reads back from Film as {film} where the file says {file}", ti.name, x.ptr.trim_start_matches(ELEMENT)));
        }
    }
    let a0 = read_anims(kind, f, &mut ti, c).map_err(|e| format!("holds \"{}\" which {e}", ti.name))?;
    let h0 = read_held(kind, &mut ti, c).map_err(|e| format!("holds \"{}\" which {e}", ti.name))?;
    Ok((ti, CutClip { origin: c.clone(), f0, a0, h0, kind, time0: (start, src, dur) }))
}

fn track_type(t: &Value) -> String {
    t.pointer("/metadata/opencut/track/type").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| {
        let lane = if schema_name(t) == "Stack" { t.pointer("/children/0").unwrap_or(t) } else { t };
        if lane.get("kind").and_then(Value::as_str) == Some("Audio") { "audio".into() } else { "video".into() }
    })
}

fn track_fields(ty: &str, s: &filmcraft_project::Sequence, lanes: &[TrackId]) -> Fields {
    let mut f = Fields::new();
    let Some(first) = lanes.first().and_then(|id| s.track(*id)) else { return f };
    if lanes.len() == 1 {
        f.insert("/name".into(), json!(first.name));
    }
    if ty == "audio" {
        f.insert("/metadata/opencut/track/muted".into(), json!(first.muted));
    } else {
        f.insert("/metadata/opencut/track/hidden".into(), json!(!first.enabled));
    }
    f
}

fn track_default(ptr: &str) -> Value {
    match ptr {
        "/name" => json!(""),
        _ => json!(false),
    }
}

impl CutParts {
    /// Film entities for Cut's parts of the file (`split`), placed in `p`; a part Film would damage is locked, named.
    pub(super) fn read(p: &mut Project, film_seqs: &[ItemId], split: Split, file: &Value) -> Result<CutParts, String> {
        let mut c = CutParts { project_mode: file.pointer("/metadata/opencut/project").is_some_and(Value::is_object), root: split.root.clone(), ..Default::default() };
        let (w, h, rate) = c.frame_of(p, film_seqs);
        c.frame = (w, h);
        let f = Frame { w: w as f64, h: h as f64 };
        // Cut's tracks in Film's timelines, after the Film track each followed in the file.
        let mut by_tl: BTreeMap<usize, Vec<(usize, Value)>> = BTreeMap::new();
        for (fi, k, t) in split.tracks {
            by_tl.entry(fi).or_default().push((k, t));
        }
        for (fi, tracks) in by_tl {
            let seq = *film_seqs.get(fi).ok_or("holds a track of Cut's in a timeline Film does not have")?;
            let order: Vec<TrackId> = p.sequence(seq).map(|s| s.video_tracks.iter().chain(s.audio_tracks.iter()).map(|t| t.id).collect()).unwrap_or_default();
            for (n, (k, t)) in tracks.into_iter().enumerate() {
                // Film's tracks before it in the file: k less the Cut tracks before it.
                let after = (k - n).checked_sub(1).and_then(|i| order.get(i)).copied();
                c.read_track(p, seq, (after, n), &t, f, rate);
            }
        }
        for (fi, m) in split.markers {
            let seq = *film_seqs.get(fi).ok_or("holds a bookmark of Cut's in a timeline Film does not have")?;
            c.read_mark(p, seq, &m)?;
        }
        let film_count = film_seqs.len();
        let mut film_seen = 0usize;
        let mut cut_at: BTreeMap<usize, Value> = split.timelines.into_iter().collect();
        let total = film_count + cut_at.len();
        let mut order = 0usize;
        for i in 0..total {
            match cut_at.remove(&i) {
                Some(tl) => {
                    let after = film_seen.checked_sub(1).and_then(|j| film_seqs.get(j)).copied();
                    c.read_scene(p, after, order, &tl, f, rate);
                    order += 1;
                }
                None => film_seen += 1,
            }
        }
        Ok(c)
    }

    /// The frame and rate Cut draws on: its project's settings, else the settings Cut saved in a Film file, else Film's first sequence's.
    fn frame_of(&self, p: &Project, film_seqs: &[ItemId]) -> (u32, u32, FrameRate) {
        let settings = self.root.get("opencut").and_then(|o| o.pointer("/project/settings").or_else(|| o.get("settings"))).filter(|s| s.is_object());
        let first = film_seqs.first().and_then(|s| p.sequence(*s)).map(|s| (s.settings.width, s.settings.height, s.settings.frame_rate));
        let size = settings.and_then(|s| Some((s.pointer("/canvasSize/width")?.as_u64()? as u32, s.pointer("/canvasSize/height")?.as_u64()? as u32)));
        let fps = settings.and_then(|s| s.get("fps")).and_then(|r| match r {
            Value::Number(n) => n.as_f64().map(rate_from_f64),
            Value::Object(o) => Some(FrameRate::new(o.get("numerator")?.as_i64()?, o.get("denominator")?.as_i64()?)),
            _ => None,
        });
        let (w, h) = size.or(first.map(|f| (f.0, f.1))).unwrap_or((1920, 1080));
        (w, h, fps.or(first.map(|f| f.2)).unwrap_or(FrameRate::FPS_30))
    }

    /// One of Cut's tracks into the sequence `seq_id`, at `(the Film track it follows, its order among Cut's)`; locked, named, when Film
    /// would damage it.
    fn read_track(&mut self, p: &mut Project, seq_id: ItemId, (after, order): (Option<TrackId>, usize), t: &Value, f: Frame, rate: FrameRate) {
        let lock = |c: &mut CutParts, reason: String| c.locked.push(Locked::Track { seq: seq_id, after, order, value: t.clone(), reason });
        let ty = track_type(t);
        let lanes: Vec<&Value> = if schema_name(t) == "Stack" { t.get("children").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default() } else { vec![t] };
        let kind = if lanes.first().and_then(|l| l.get("kind")).and_then(Value::as_str) == Some("Audio") { TrackKind::Audio } else { TrackKind::Video };
        for (k, x) in [("source_range", Value::Null), ("enabled", json!(true))] {
            if lanes.iter().chain(std::iter::once(&t)).any(|l| l.get(k).is_some_and(|v| !same(v, &x))) {
                return lock(self, format!("its {k} is set, which Cut does not show either"));
            }
        }
        if lanes.iter().chain(std::iter::once(&t)).any(|l| l.get("effects").and_then(Value::as_array).is_some_and(|e| !e.is_empty()) || l.get("markers").and_then(Value::as_array).is_some_and(|m| !m.is_empty())) {
            return lock(self, "it carries effects or markers of its own".into());
        }
        let Some(mut seq) = p.sequence(seq_id).cloned() else { return };
        let mut ids = Vec::new();
        let mut clips = Vec::new();
        let switches = t.pointer("/metadata/opencut/track").cloned().unwrap_or(json!({}));
        for l in &lanes {
            let id = TrackId(p.alloc_id());
            let mut tr = Track::new(id, kind, l.get("name").and_then(Value::as_str).unwrap_or("").to_string());
            if ty == "audio" {
                tr.muted = switches.get("muted").and_then(Value::as_bool).unwrap_or(false);
            } else {
                tr.enabled = !switches.get("hidden").and_then(Value::as_bool).unwrap_or(false);
            }
            let mut pos = Tick::ZERO;
            for c in l.get("children").and_then(Value::as_array).into_iter().flatten() {
                match schema_name(c) {
                    "Gap" => match c.get("source_range").and_then(read_time) {
                        Some((_, d)) => pos += d,
                        None => return lock(self, "it holds a gap without a length".into()),
                    },
                    "Clip" => {
                        let Some((src, dur)) = c.get("source_range").and_then(read_time) else { return lock(self, "it holds a clip without a source range".into()) };
                        match read_clip(p, c, kind, f, rate, (pos, src, dur)) {
                            Ok((ti, cc)) => {
                                clips.push((ti.id, cc));
                                tr.items.push(ti);
                            }
                            Err(why) => return lock(self, format!("it {why}")),
                        }
                        pos += dur;
                    }
                    other => return lock(self, format!("it holds a {other}")),
                }
            }
            tr.sort();
            ids.push(id);
            seq.tracks_mut(kind).push(tr);
        }
        let f0 = track_fields(&ty, &seq, &ids);
        for (ptr, v) in &f0 {
            let file = t.pointer(ptr).cloned().unwrap_or_else(|| track_default(ptr));
            if !same(&file, v) {
                return lock(self, format!("its {ptr} reads back from Film as {v} where the file says {file}"));
            }
        }
        if let Some(item) = p.item_mut(seq_id) {
            item.kind = ItemKind::Sequence(Arc::new(seq));
        }
        self.clips.extend(clips);
        self.tracks.push(CutTrack { seq: seq_id, lanes: ids, kind, after, order, origin: t.clone(), f0, ty });
    }

    fn read_mark(&mut self, p: &mut Project, seq_id: ItemId, m: &Value) -> Result<(), String> {
        let (start, duration) = m.get("marked_range").and_then(read_time).ok_or("holds a bookmark without a time")?;
        let b = m.pointer("/metadata/opencut/bookmark").cloned().unwrap_or(json!({}));
        let id = MarkerId(p.alloc_id());
        let marker = Marker {
            url: String::new(),
            author: String::new(),
            custom_color: b.get("color").and_then(Value::as_str).and_then(HexColor::parse),
            id,
            start,
            duration,
            name: m.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            comment: String::new(),
            kind: MarkerKind::default(),
            color: Label::Rose,
        };
        let f0 = mark_fields(&marker, m);
        for (ptr, v) in &f0 {
            let file = m.pointer(ptr).cloned().unwrap_or(Value::Null);
            if !same(&file, v) {
                return Err(format!("holds the bookmark \"{}\" whose {ptr} reads back from Film as {v} where the file says {file}", marker.name));
            }
        }
        if let Some(s) = p.sequence_mut(seq_id) {
            s.markers.push(marker);
            s.markers.sort_by_key(|m| m.start);
        }
        self.marks.insert(id, Mark { origin: m.clone(), f0, time0: start });
        Ok(())
    }

    fn read_scene(&mut self, p: &mut Project, after: Option<ItemId>, order: usize, tl: &Value, f: Frame, rate: FrameRate) {
        let name = tl.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let settings = settings_for(rate, f.w as u32, f.h as u32, false);
        let seq = p.add_item(&name, Label::Forest, ItemKind::Sequence(Arc::new(empty_sequence(settings))), None);
        let stack = tl.get("tracks").cloned().unwrap_or(json!({}));
        for (k, t) in stack.get("children").and_then(Value::as_array).into_iter().flatten().enumerate() {
            self.read_track(p, seq, (None, k), t, f, rate);
        }
        for m in stack.get("markers").and_then(Value::as_array).into_iter().flatten() {
            if self.read_mark(p, seq, m).is_err() {
                // A scene whose bookmark Film cannot hold is carried whole.
                let _ = self.unread_scene(p, seq);
                self.locked.push(Locked::Timeline { after, order, value: tl.clone(), reason: "a bookmark of it does not read back from Film exactly".into() });
                return;
            }
        }
        self.scenes.push(Scene { seq, after, order, origin: tl.clone(), name0: name });
    }

    fn unread_scene(&mut self, p: &mut Project, seq: ItemId) -> Option<()> {
        let s = p.sequence(seq)?.clone();
        for t in s.video_tracks.iter().chain(s.audio_tracks.iter()) {
            for it in &t.items {
                self.clips.remove(&it.id);
            }
        }
        for m in &s.markers {
            self.marks.remove(&m.id);
        }
        self.tracks.retain(|t| t.seq != seq);
        self.locked.retain(|l| !matches!(l, Locked::Track { seq: s, .. } if *s == seq));
        p.items.remove(&seq);
        remove_from_bins(&mut p.root, seq);
        Some(())
    }

    /// The sequence to show first when Film's own part has none.
    pub fn first_sequence(&self) -> Option<ItemId> {
        self.scenes.iter().min_by_key(|s| s.order).map(|s| s.seq)
    }

    /// `p` less Cut's parts: Film's own project as Film writes it.
    pub(super) fn strip(&self, p: &Project, film_items: &BTreeSet<ItemId>) -> Project {
        let mut q = p.clone();
        for s in &self.scenes {
            q.items.remove(&s.seq);
            remove_from_bins(&mut q.root, s.seq);
        }
        let lanes: BTreeSet<TrackId> = self.tracks.iter().filter(|t| !self.scenes.iter().any(|s| s.seq == t.seq)).flat_map(|t| t.lanes.iter().copied()).collect();
        let ids: Vec<ItemId> = q.items.keys().copied().collect();
        for id in ids {
            if let Some(s) = q.sequence_mut(id) {
                s.video_tracks.retain(|t| !lanes.contains(&t.id));
                s.audio_tracks.retain(|t| !lanes.contains(&t.id));
                s.markers.retain(|m| !self.marks.contains_key(&m.id));
            }
        }
        // Items Film made for Cut's parts (a medium, a graphic, an adjustment layer) that nothing of Film's own uses.
        let used: BTreeSet<ItemId> =
            q.items.values().filter_map(|i| i.as_sequence()).flat_map(|s| s.video_tracks.iter().chain(s.audio_tracks.iter())).flat_map(|t| t.items.iter().map(|it| it.item)).collect();
        let unused: Vec<ItemId> = q.items.keys().filter(|id| !film_items.contains(id) && !used.contains(id) && q.items[id].as_sequence().is_none()).copied().collect();
        for id in unused {
            q.items.remove(&id);
            remove_from_bins(&mut q.root, id);
        }
        q
    }
}

fn remove_from_bins(b: &mut filmcraft_project::Bin, id: ItemId) {
    b.children.retain(|e| !matches!(e, filmcraft_project::BinEntry::Item(i) if *i == id));
    for e in &mut b.children {
        if let filmcraft_project::BinEntry::Bin(sub) = e {
            remove_from_bins(sub, id);
        }
    }
}

fn mark_fields(m: &Marker, origin: &Value) -> Fields {
    let b = origin.pointer("/metadata/opencut/bookmark");
    let had = |k: &str| b.and_then(|b| b.get(k)).is_some();
    let mut f = Fields::new();
    f.insert("/name".into(), json!(m.name));
    f.insert("/metadata/opencut/bookmark/note".into(), if !m.name.is_empty() || had("note") { json!(m.name) } else { Value::Null });
    f.insert("/metadata/opencut/bookmark/color".into(), m.custom_color.map(|c| json!(format!("#{:02x}{:02x}{:02x}", c.0[0], c.0[1], c.0[2]))).unwrap_or(Value::Null));
    f.insert(
        "/metadata/opencut/bookmark/duration".into(),
        match cut_ticks(m.duration) {
            Some(d) if d != 0 || had("duration") => json!(d),
            Some(_) => Value::Null,
            None => json!(m.duration.0 as f64 / PER_CUT_TICK as f64),
        },
    );
    f
}

// ---------------------------------------------------------------------------------------------
// Writing Cut's parts back in Cut's shape
// ---------------------------------------------------------------------------------------------

/// What one of Film's clips on a track of Cut's is, as one of Cut's elements (`None`: it has no place in Cut's shape).
fn kind_of_clip(p: &Project, ti: &TrackItem, track_kind: TrackKind) -> Result<Kind, String> {
    let item = p.item(ti.item).ok_or("a clip of a missing item")?;
    match &item.kind {
        ItemKind::Graphic { .. } => {
            let layers: Vec<&EffectInstance> = ti.effects.iter().filter(|e| graphic::is_layer(e)).collect();
            match layers.as_slice() {
                [l] if l.effect == graphic::TEXT_LAYER => Ok(Kind::Text),
                [l] if l.effect == graphic::SHAPE_LAYER && matches!(l.params.get("shape").map(|p| &p.value), Some(ParamValue::Choice(0..=2))) => Ok(Kind::Shape),
                _ => Err(format!("element-not-in-cut: the graphic \"{}\" holds layers Cut's text or shape element cannot hold (one text, or one rectangle, ellipse or polygon)", ti.name)),
            }
        }
        ItemKind::AdjustmentLayer { .. } => {
            let own: Vec<&EffectInstance> = ti.effects.iter().filter(|e| !matches!(e.effect.as_str(), "motion" | "opacity" | "time_remap")).collect();
            match own.as_slice() {
                [e] if e.effect == "gaussian_blur" => Ok(Kind::Effect),
                _ => Err(format!("element-not-in-cut: the adjustment layer \"{}\" holds effects other than one Gaussian Blur, which Cut's effect element cannot hold", ti.name)),
            }
        }
        ItemKind::Media(m) => match &m.media {
            MediaRef::File { path } if is_oneos_media(path) => Ok(if track_kind == TrackKind::Audio {
                Kind::Audio
            } else if matches!(m.info.kind, filmcraft_media::MediaKind::Still) {
                Kind::Image
            } else {
                Kind::Video
            }),
            _ => Err(format!("media-not-referenced: \"{}\" is not a OneOS file", ti.name)),
        },
        _ => Err(format!("element-not-in-cut: \"{}\" (a nested sequence or subclip) has no place on a track of Cut's", ti.name)),
    }
}

fn media_url(p: &Project, ti: &TrackItem) -> Option<(String, String)> {
    let i = p.item(ti.item)?;
    match &i.as_media()?.media {
        MediaRef::File { path } => Some((path.clone(), i.name.clone())),
        _ => None,
    }
}

/// The media list entry of a OneOS medium in a Cut project (as Cut keys one it did not import: `oneos-<id>-<version>`).
fn media_entry(url: &str, name: &str, kind: Kind) -> Option<(String, Value)> {
    let rest = url.strip_prefix("oneos:file/")?;
    let (id, version) = rest.split_once("?version=")?;
    let key = format!("oneos-{id}-{version}");
    Some((key.clone(), json!({"id": key, "name": name, "type": if kind == Kind::Audio { "audio" } else if kind == Kind::Image { "image" } else { "video" }, "file": {"id": id, "version": version}})))
}

struct Out<'a> {
    p: &'a Project,
    parts: &'a CutParts,
    r: Rendered,
}

impl Out<'_> {
    fn frame(&self) -> Frame {
        Frame { w: self.parts.frame.0 as f64, h: self.parts.frame.1 as f64 }
    }

    fn clip(&mut self, ti: &TrackItem, track_kind: TrackKind, lane_rate: f64) -> Result<Value, String> {
        let f = self.frame();
        let prior = self.parts.clips.get(&ti.id);
        let kind = match prior {
            Some(c) => c.kind,
            None => kind_of_clip(self.p, ti, track_kind)?,
        };
        let empty = json!({});
        let origin = prior.map(|c| &c.origin).unwrap_or(&empty);
        let now = read_fields(kind, f, ti, origin).map_err(|e| format!("\"{}\" {e}", ti.name))?;
        let time = (ti.start, ti.source_in, ti.duration);
        let mut c = match prior {
            Some(pr) => {
                let mut c = pr.origin.clone();
                let changed: Vec<&String> = now.keys().filter(|k| !pr.f0.get(*k).is_some_and(|v| same(v, &now[*k]))).collect();
                // Interim until step 2b: a video element's sound is Cut's own (its volume channel has no place on Film's video clip until it is
                // Film's linked audio), so Film does not change the volume Cut animates.
                if kind == Kind::Video
                    && changed.iter().any(|k| k.ends_with("/volume"))
                    && c.pointer(&format!("{ELEMENT}/animations/bindings/volume")).is_some()
                {
                    return Err(format!("volume-keyframed-in-cut: \"{}\"'s volume is animated in Cut, which Film shows on a video clip once its sound is linked audio", ti.name));
                }
                for k in changed {
                    put(&mut c, k, now[k].clone());
                }
                let a = write_anims(kind, f, ti, &mut c, &pr.a0, &ti.name)?;
                let h = write_held(kind, ti, &mut c, &pr.h0, &ti.name)?;
                self.r.clips.insert(ti.id, CutClip { origin: Value::Null, f0: Fields::new(), a0: a, h0: h, kind, time0: time });
                if (pr.time0.1, pr.time0.2) != (time.1, time.2) {
                    let rate = c.pointer("/source_range/duration/rate").and_then(Value::as_f64).unwrap_or(lane_rate);
                    c["source_range"] = time_range(ti.source_in, ti.duration, rate)?;
                }
                if ti.name != c.get("name").and_then(Value::as_str).unwrap_or("") {
                    c["name"] = json!(ti.name);
                    if kind.generated() {
                        put(&mut c, "/media_references/DEFAULT_MEDIA/name", json!(ti.name));
                    }
                }
                c
            }
            None => {
                let mut c = self.fresh(ti, kind, &now, lane_rate)?;
                let a = write_anims(kind, f, ti, &mut c, &Anims::new(), &ti.name)?;
                let h = write_held(kind, ti, &mut c, &Held::new(), &ti.name)?;
                self.r.clips.insert(ti.id, CutClip { origin: Value::Null, f0: Fields::new(), a0: a, h0: h, kind, time0: time });
                c
            }
        };
        if !c.is_object() {
            c = json!({});
        }
        let (a0, h0) = self.r.clips.remove(&ti.id).map(|x| (x.a0, x.h0)).unwrap_or_default();
        self.r.clips.insert(ti.id, CutClip { origin: c.clone(), f0: now, a0, h0, kind, time0: time });
        Ok(c)
    }

    /// One of Film's clips written as Cut writes a new element.
    fn fresh(&mut self, ti: &TrackItem, kind: Kind, now: &Fields, rate: f64) -> Result<Value, String> {
        let range = time_range(ti.source_in, ti.duration, rate)?;
        let mut c = json!({"OTIO_SCHEMA": "Clip.2", "name": ti.name, "source_range": range, "effects": [], "markers": [], "enabled": true, "media_references": {}, "active_media_reference_key": "DEFAULT_MEDIA", "metadata": {"opencut": {"element": {"trimEnd": 0}}}});
        for (k, v) in now {
            put(&mut c, k, v.clone());
        }
        let id = format!("film-{}", ti.id.0);
        let el = ELEMENT;
        if self.parts.project_mode {
            put(&mut c, &format!("{el}/id"), json!(id));
            put(&mut c, &format!("{el}/type"), json!(kind.cut_type()));
        }
        match kind {
            Kind::Shape => {
                let shape = match layer(ti).and_then(|l| l.params.get("shape")).map(|p| &p.value) {
                    Some(ParamValue::Choice(i)) => *i as usize,
                    _ => 0,
                };
                put(&mut c, &format!("{el}/definitionId"), json!(SHAPES[shape.min(2)]));
            }
            Kind::Effect => put(&mut c, &format!("{el}/effectType"), json!("blur")),
            Kind::Audio if self.parts.project_mode => put(&mut c, &format!("{el}/sourceType"), json!("upload")),
            _ => {}
        }
        if kind.generated() {
            c["media_references"]["DEFAULT_MEDIA"] = json!({"OTIO_SCHEMA": "GeneratorReference.1", "name": ti.name, "generator_kind": format!("OpenCut.{}", kind.cut_type()), "parameters": {}, "available_range": null, "metadata": {}});
        } else {
            let (url, name) = media_url(self.p, ti).ok_or_else(|| format!("media-not-referenced: \"{}\" is not a OneOS file", ti.name))?;
            if self.parts.project_mode {
                let (key, entry) = media_entry(&url, &name, kind).ok_or_else(|| format!("media-not-referenced: \"{}\" is not a OneOS file", ti.name))?;
                put(&mut c, &format!("{el}/mediaId"), json!(key));
                self.r.media.push(entry);
            }
            c["media_references"]["DEFAULT_MEDIA"] = json!({"OTIO_SCHEMA": "ExternalReference.1", "name": name, "target_url": url, "available_range": null, "available_image_bounds": null, "metadata": {}});
        }
        Ok(c)
    }

    /// A lane: Film's clips placed by gaps (a gap the file held at the same place and length is its own bytes).
    fn lane(&mut self, items: &[TrackItem], kind: TrackKind, was: &[Value], rate: f64) -> Result<Vec<Value>, String> {
        let mut gaps = Vec::new();
        let mut at = Tick::ZERO;
        for c in was {
            let d = c.get("source_range").and_then(read_time).map(|r| r.1).unwrap_or(Tick::ZERO);
            if schema_name(c) == "Gap" {
                gaps.push((at, d, c.clone()));
            }
            at += d;
        }
        let mut out = Vec::new();
        let mut at = Tick::ZERO;
        let mut sorted: Vec<&TrackItem> = items.iter().collect();
        sorted.sort_by_key(|i| i.start);
        for ti in sorted {
            if ti.start > at {
                let g = ti.start - at;
                match gaps.iter().find(|(a, d, _)| *a == at && *d == g) {
                    Some((_, _, raw)) => out.push(raw.clone()),
                    None => out.push(json!({"OTIO_SCHEMA": "Gap.1", "name": "", "source_range": time_range(Tick::ZERO, g, rate)?, "effects": [], "markers": [], "enabled": true, "metadata": {}})),
                }
            }
            out.push(self.clip(ti, kind, rate)?);
            at = ti.end();
        }
        Ok(out)
    }

    /// A track of Cut's from Film's track(s), in Cut's shape.
    fn track(&mut self, seq: &filmcraft_project::Sequence, ct: Option<&CutTrack>, lanes: &[TrackId], kind: TrackKind) -> Result<Value, String> {
        let first = lanes.first().and_then(|id| seq.track(*id)).ok_or("a track of Cut's is gone")?;
        let ty = match ct {
            Some(t) => t.ty.clone(),
            None => {
                let kinds: BTreeSet<&str> = first.items.iter().map(|it| kind_of_clip(self.p, it, kind).map(Kind::cut_type)).collect::<Result<_, _>>()?;
                match kinds.len() {
                    0 => (if kind == TrackKind::Audio { "audio" } else { "video" }).to_string(),
                    1 => match *kinds.iter().next().unwrap_or(&"video") {
                        "image" => "video".into(),
                        "graphic" => "graphic".into(),
                        other => other.into(),
                    },
                    _ => return Err(format!("track-not-in-cut: the track \"{}\" holds {} kinds of element; a track of Cut's holds one", first.name, kinds.len())),
                }
            }
        };
        let rate_of = |v: &Value| -> f64 {
            v.get("children").and_then(Value::as_array).and_then(|c| c.iter().find_map(|x| x.pointer("/source_range/duration/rate").and_then(Value::as_f64))).unwrap_or(CUT_TICKS as f64)
        };
        let mut body = match ct {
            Some(t) => t.origin.clone(),
            None => {
                let mut meta = json!({"opencut": {"track": {"type": ty}}});
                if ty == "audio" {
                    meta["opencut"]["track"]["muted"] = json!(false);
                } else {
                    if ty == "video" {
                        meta["opencut"]["track"]["muted"] = json!(false);
                    }
                    meta["opencut"]["track"]["hidden"] = json!(false);
                }
                if self.parts.project_mode {
                    meta["opencut"]["track"]["id"] = json!(format!("film-{}", first.id.0));
                    meta["opencut"]["slot"] = json!(if ty == "audio" { "audio" } else { "overlay" });
                    meta["opencut"]["order"] = json!([]);
                }
                json!({"OTIO_SCHEMA": "Track.1", "name": first.name, "kind": if kind == TrackKind::Audio { "Audio" } else { "Video" }, "source_range": null, "effects": [], "markers": [], "enabled": true, "metadata": meta, "children": []})
            }
        };
        let stacked = schema_name(&body) == "Stack";
        if stacked != (lanes.len() > 1) && ct.is_some() && lanes.len() > 1 {
            return Err("track-not-in-cut: a track of Cut's was split into lanes in Film".into());
        }
        let mut ids_in_order = Vec::new();
        if stacked {
            let was: Vec<Value> = body.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut out = Vec::new();
            for (i, id) in lanes.iter().enumerate() {
                let tr = seq.track(*id).ok_or("a lane of Cut's is gone")?;
                let mut l = was.get(i).cloned().unwrap_or_else(|| json!({"OTIO_SCHEMA": "Track.1", "name": format!("{} (lane {})", first.name, i + 1), "kind": body.pointer("/children/0/kind").cloned().unwrap_or(json!("Video")), "source_range": null, "effects": [], "markers": [], "enabled": true, "metadata": {"opencut": {"lane": i}}, "children": []}));
                let lw: Vec<Value> = l.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
                let rate = rate_of(&l);
                l["children"] = Value::Array(self.lane(&tr.items, kind, &lw, rate)?);
                if tr.name != l.get("name").and_then(Value::as_str).unwrap_or("") {
                    l["name"] = json!(tr.name);
                }
                ids_in_order.extend(tr.items.iter().map(|it| it.id));
                out.push(l);
            }
            body["children"] = Value::Array(out);
        } else {
            let was: Vec<Value> = body.get("children").and_then(Value::as_array).cloned().unwrap_or_default();
            let rate = rate_of(&body);
            body["children"] = Value::Array(self.lane(&first.items, kind, &was, rate)?);
            ids_in_order.extend(first.items.iter().map(|it| it.id));
        }
        let now = track_fields(&ty, seq, lanes);
        for (k, v) in &now {
            let changed = match ct {
                Some(t) => !t.f0.get(k).is_some_and(|w| same(w, v)),
                None => true,
            };
            if changed {
                put(&mut body, k, v.clone());
            }
        }
        // A Cut project lists a track's elements by id.
        if self.parts.project_mode && body.pointer("/metadata/opencut/order").is_some() {
            let was: Vec<Value> = body.pointer("/metadata/opencut/order").and_then(Value::as_array).cloned().unwrap_or_default();
            let el_id = |id: &ClipId| self.r.clips.get(id).and_then(|c| c.origin.pointer(&format!("{ELEMENT}/id")).cloned());
            let present: Vec<Value> = ids_in_order.iter().filter_map(el_id).collect();
            let mut order: Vec<Value> = was.into_iter().filter(|w| present.iter().any(|p| same(p, w))).collect();
            for p in present {
                if !order.iter().any(|o| same(o, &p)) {
                    order.push(p);
                }
            }
            put(&mut body, "/metadata/opencut/order", Value::Array(order));
        }
        Ok(body)
    }

    fn marks(&mut self, seq: &filmcraft_project::Sequence, only_origin: bool) -> Result<Vec<Value>, String> {
        let mut out = Vec::new();
        for m in &seq.markers {
            let prior = self.parts.marks.get(&m.id);
            if only_origin && prior.is_none() {
                continue;
            }
            let empty = json!({});
            let now = mark_fields(m, prior.map(|x| &x.origin).unwrap_or(&empty));
            let v = match prior {
                Some(pr) => {
                    let mut v = pr.origin.clone();
                    for (k, x) in &now {
                        if !pr.f0.get(k).is_some_and(|w| same(w, x)) {
                            put(&mut v, k, x.clone());
                        }
                    }
                    if pr.time0 != m.start {
                        let rate = v.pointer("/marked_range/start_time/rate").and_then(Value::as_f64).unwrap_or(CUT_TICKS as f64);
                        v["marked_range"] = time_range(m.start, m.duration, rate)?;
                    }
                    v
                }
                None => {
                    let mut v = json!({"OTIO_SCHEMA": "Marker.2", "name": m.name, "marked_range": time_range(m.start, m.duration, CUT_TICKS as f64)?, "color": "RED", "comment": "", "metadata": {"opencut": {"bookmark": {}}}});
                    for (k, x) in &now {
                        put(&mut v, k, x.clone());
                    }
                    v
                }
            };
            self.r.marks.insert(m.id, Mark { origin: v.clone(), f0: now, time0: m.start });
            out.push(v);
        }
        Ok(out)
    }

    fn scene(&mut self, s: &Scene) -> Result<Value, String> {
        let seq = self.p.sequence(s.seq).ok_or("a scene of Cut's is gone")?;
        let mut tl = s.origin.clone();
        let name = self.p.item(s.seq).map(|i| i.name.clone()).unwrap_or_default();
        if name != s.name0 {
            tl["name"] = json!(name);
        }
        // Its tracks in their order: Cut's (and the locked ones) where the file held them, then the ones Film made that hold something.
        let mut placed: Vec<(usize, Value)> = Vec::new();
        let mut held: BTreeSet<TrackId> = BTreeSet::new();
        for ct in self.parts.tracks.iter().filter(|t| t.seq == s.seq) {
            if ct.lanes.iter().all(|id| seq.track(*id).is_none()) {
                continue;
            }
            held.extend(ct.lanes.iter().copied());
            let body = self.track(seq, Some(ct), &ct.lanes, ct.kind)?;
            self.r.tracks.push(CutTrack { origin: body.clone(), f0: track_fields(&ct.ty, seq, &ct.lanes), ..ct.clone() });
            placed.push((ct.order, body));
        }
        for l in &self.parts.locked {
            if let Locked::Track { seq: q, order, value, .. } = l
                && *q == s.seq
            {
                placed.push((*order, value.clone()));
            }
        }
        placed.sort_by_key(|(o, _)| *o);
        let mut next = placed.last().map(|(o, _)| o + 1).unwrap_or(0);
        for kind in [TrackKind::Video, TrackKind::Audio] {
            for t in seq.tracks(kind) {
                if held.contains(&t.id) || t.items.is_empty() {
                    continue;
                }
                let body = self.track(seq, None, &[t.id], kind)?;
                let ty = track_type(&body);
                self.r.tracks.push(CutTrack { seq: s.seq, lanes: vec![t.id], kind, after: None, order: next, origin: body.clone(), f0: track_fields(&ty, seq, &[t.id]), ty });
                placed.push((next, body));
                next += 1;
            }
        }
        tl["tracks"]["children"] = Value::Array(placed.into_iter().map(|(_, b)| b).collect());
        let marks = self.marks(seq, false)?;
        tl["tracks"]["markers"] = Value::Array(marks);
        self.r.scenes.push(Scene { origin: tl.clone(), name0: name, ..s.clone() });
        Ok(tl)
    }
}

impl CutParts {
    /// Put Cut's parts, in Cut's shape, into `doc` (Film's own part as Film writes it; `film_seqs` its timelines in order).
    pub(super) fn place(&self, p: &Project, film_seqs: &[ItemId], doc: &mut Value) -> Result<Rendered, String> {
        let mut o = Out { p, parts: self, r: Rendered::default() };
        for (i, seq_id) in film_seqs.iter().enumerate() {
            let Some(seq) = p.sequence(*seq_id) else { continue };
            let order: Vec<TrackId> = seq.video_tracks.iter().chain(seq.audio_tracks.iter()).map(|t| t.id).filter(|id| !self.tracks.iter().any(|t| t.lanes.contains(id))).collect();
            let mut inserts: Vec<(Option<TrackId>, usize, Value)> = Vec::new();
            for ct in self.tracks.iter().filter(|t| t.seq == *seq_id) {
                if ct.lanes.iter().all(|id| seq.track(*id).is_none()) {
                    continue;
                }
                let body = o.track(seq, Some(ct), &ct.lanes, ct.kind)?;
                o.r.tracks.push(CutTrack { origin: body.clone(), f0: track_fields(&ct.ty, seq, &ct.lanes), ..ct.clone() });
                inserts.push((ct.after, ct.order, body));
            }
            for l in &self.locked {
                if let Locked::Track { seq: q, after, order, value, .. } = l
                    && q == seq_id
                {
                    inserts.push((*after, *order, value.clone()));
                }
            }
            inserts.sort_by_key(|(_, n, _)| *n);
            let marks = o.marks(seq, true)?;
            let Some(tl) = doc.get_mut("children").and_then(|c| c.get_mut(i)) else { continue };
            let stack = &mut tl["tracks"];
            let children = stack["children"].as_array_mut().ok_or("Film's timeline has no tracks")?;
            // After the Film track each followed (the first when it led, the end when that track is gone), in the file's order.
            let mut placed: Vec<Value> = Vec::new();
            let mut film = std::mem::take(children).into_iter();
            let lead: Vec<Value> = inserts.iter().filter(|(a, _, _)| a.is_none()).map(|(_, _, b)| b.clone()).collect();
            placed.extend(lead);
            for id in &order {
                if let Some(t) = film.next() {
                    placed.push(t);
                }
                placed.extend(inserts.iter().filter(|(a, _, _)| *a == Some(*id)).map(|(_, _, b)| b.clone()));
            }
            placed.extend(film);
            placed.extend(inserts.iter().filter(|(a, _, _)| a.is_some_and(|a| !order.contains(&a))).map(|(_, _, b)| b.clone()));
            *children = placed;
            if !marks.is_empty() {
                let ms = stack.as_object_mut().ok_or("Film's stack is not an object")?.entry("markers").or_insert_with(|| json!([]));
                if let Value::Array(a) = ms {
                    a.extend(marks);
                }
            }
        }
        // Cut's scenes and the locked timelines, after the Film timeline each followed.
        let mut timelines: Vec<(Option<ItemId>, usize, Value)> = Vec::new();
        for s in &self.scenes {
            if p.sequence(s.seq).is_none() {
                continue;
            }
            timelines.push((s.after, s.order, o.scene(s)?));
        }
        for l in &self.locked {
            if let Locked::Timeline { after, order, value, .. } = l {
                timelines.push((*after, *order, value.clone()));
            }
        }
        timelines.sort_by_key(|(_, n, _)| *n);
        if !timelines.is_empty() {
            let children = doc.get_mut("children").and_then(Value::as_array_mut).ok_or("Film's file has no timelines")?;
            let mut film = std::mem::take(children).into_iter();
            let mut placed: Vec<Value> = timelines.iter().filter(|(a, _, _)| a.is_none()).map(|(_, _, t)| t.clone()).collect();
            for id in film_seqs {
                if let Some(t) = film.next() {
                    placed.push(t);
                }
                placed.extend(timelines.iter().filter(|(a, _, _)| *a == Some(*id)).map(|(_, _, t)| t.clone()));
            }
            placed.extend(film);
            placed.extend(timelines.iter().filter(|(a, _, _)| a.is_some_and(|a| !film_seqs.contains(&a))).map(|(_, _, t)| t.clone()));
            *children = placed;
        }
        // The root's metadata other than Film's, with the media a Cut project now names.
        if !self.root.is_empty() {
            let mut root = self.root.clone();
            self.parts_media(&mut root, &o.r.media);
            let meta = doc.as_object_mut().ok_or("Film's file is not an object")?.entry("metadata").or_insert_with(|| json!({}));
            if let Value::Object(m) = meta {
                for (k, v) in root {
                    m.insert(k, v);
                }
            }
        }
        Ok(o.r)
    }

    fn parts_media(&self, root: &mut Map<String, Value>, added: &[Value]) -> bool {
        let Some(Value::Array(media)) = root.get_mut("opencut").and_then(|o| o.get_mut("media")) else { return false };
        let mut changed = false;
        for m in added {
            if !media.iter().any(|x| x.get("id") == m.get("id")) {
                media.push(m.clone());
                changed = true;
            }
        }
        changed
    }

    /// The file now holds what the last render made: it is what the next save is measured against.
    pub(super) fn adopt(&mut self, r: Rendered) {
        if !r.media.is_empty() {
            let mut root = self.root.clone();
            self.parts_media(&mut root, &r.media);
            self.root = root;
        }
        self.scenes = r.scenes;
        self.tracks = r.tracks;
        self.clips = r.clips;
        self.marks = r.marks;
    }
}
