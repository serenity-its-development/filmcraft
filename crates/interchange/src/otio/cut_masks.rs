//! Cut's masks as Film's masks, and back in Cut's shape (OneOS doctrine files/989, "Masks, both ways").
//!
//! Cut's masks are parametric and sized to the element (OpenCut `masks/definitions`): a split, cinematic bars, a rectangle, an ellipse, a
//! heart, a diamond, a star (fractions of the element), a custom path (points in fractions of the element), a text. Film's are closed
//! cubic Bézier paths in clip pixels ([`MaskPath`]). With the element's own pixels placed in the clip ([`ElementBox`]: a medium's pixel
//! size, which the file carries, or the square Film draws a graphic of Cut's on), each of Cut's masks is the path its definition draws
//! (pinned against the editor's own `buildPath`, `tests/it/oneos/mask-paths.json`). Both draw a mask the same way: feathered across its
//! edge after it is grown by its expansion, made `opacity · (inverted ? 1 − c : c)`, combined top to bottom by its mode (Cut's
//! `masks/combine.ts` is Film's [`MaskMode`]). Cut measures a mask's feather and expansion in the canvas's pixels (its own masks) or in
//! clip pixels (`units: "clip"`, the masks Film draws), and so does Film ([`MaskUnits`], Mona 2026-11-09: "Build canvas feather in
//! Film"): a mask of Cut's in canvas pixels is Film's canvas mask with the same numbers, drawn through the clip's Motion frame by frame,
//! on any scale, keyed or uneven.
//!
//! A save writes each mask back in Cut's shape: a mask Film did not change exactly as the file held it; a changed one with only what
//! changed rewritten (a path Film reshaped becomes Cut's custom path of the same curve, a feather or expansion Film set is written in
//! the mask's own units); a mask Film added as Cut's custom path. What Cut has no place for is refused by name: keyframes on a mask
//! (`mask-keyframes-not-in-cut`), a mask on an effect rather than the clip (`effect-mask-not-in-cut`).

use std::f64::consts::PI;

use filmcraft_geom::Vec2;
use filmcraft_project::{Mask, MaskMode, MaskPath, MaskUnits, MaskVertex, Param, ParamValue, TrackMethod};
use serde_json::{Map, Value, json};

/// The element's own pixels in Film's clip pixels (where Cut's fractions of the element are).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ElementBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Film's circle tangent (the same as [`filmcraft_project::mask::KAPPA`]).
const KAPPA: f64 = filmcraft_project::mask::KAPPA;

fn num(v: &Value, k: &str, d: f64) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(d)
}

fn rotate_about(x: f64, y: f64, cx: f64, cy: f64, rot: f64) -> Vec2 {
    let (dx, dy) = (x - cx, y - cy);
    let (c, s) = (rot.cos(), rot.sin());
    Vec2::new(cx + dx * c - dy * s, cy + dx * s + dy * c)
}

fn polygon(points: Vec<Vec2>) -> MaskPath {
    MaskPath { vertices: points.into_iter().map(MaskVertex::corner).collect(), closed: true }
}

/// The geometry of Cut's box-like masks (OpenCut `definitions/box-like.ts` `getBoxLikeGeometry`).
fn box_like(p: &Value, w: f64, h: f64) -> (f64, f64, f64, f64, f64) {
    let cx = w / 2.0 + num(p, "centerX", 0.0) * w;
    let cy = h / 2.0 + num(p, "centerY", 0.0) * h;
    let mw = num(p, "width", 0.6).max(0.01) * w;
    let mh = num(p, "height", 0.6).max(0.01) * h;
    (cx, cy, mw, mh, num(p, "rotation", 0.0) * PI / 180.0)
}

fn rect(cx: f64, cy: f64, hw: f64, hh: f64, rot: f64) -> MaskPath {
    polygon([(cx - hw, cy - hh), (cx + hw, cy - hh), (cx + hw, cy + hh), (cx - hw, cy + hh)].into_iter().map(|(x, y)| rotate_about(x, y, cx, cy, rot)).collect())
}

/// A split's half-plane (the side its normal points), cut to a box `w + h` beyond every edge of the element (OpenCut
/// `definitions/split.ts`), so growing or shrinking it moves only its line.
fn split(p: &Value, w: f64, h: f64) -> MaskPath {
    let m = w + h;
    let (bw, bh) = (w + 2.0 * m, h + 2.0 * m);
    let (cx, cy) = (num(p, "centerX", 0.0) * w / bw, num(p, "centerY", 0.0) * h / bh);
    let a = num(p, "rotation", 0.0) * PI / 180.0;
    let snap = |x: f64| if x.abs() < 1e-10 { 0.0 } else { x };
    let (nx, ny) = (snap(a.cos()), snap(a.sin()));
    let (lx, ly) = (bw / 2.0 + cx * bw, bh / 2.0 + cy * bh);
    let side = |x: f64, y: f64| (x - lx) * nx + (y - ly) * ny;
    let hit = |x1: f64, y1: f64, x2: f64, y2: f64| {
        let (d1, d2) = (side(x1, y1), side(x2, y2));
        let den = d1 - d2;
        if den.abs() < 1e-10 {
            return None;
        }
        let t = d1 / den;
        (0.0..=1.0).contains(&t).then_some((x1 + (x2 - x1) * t, y1 + (y2 - y1) * t))
    };
    let mut v: Vec<(f64, f64)> = Vec::new();
    for (x1, y1, x2, y2) in [(0.0, 0.0, bw, 0.0), (bw, 0.0, bw, bh), (bw, bh, 0.0, bh), (0.0, bh, 0.0, 0.0)] {
        let (i1, i2) = (side(x1, y1) >= 0.0, side(x2, y2) >= 0.0);
        if i1 && i2 {
            v.push((x2, y2));
        } else if i1 {
            v.extend(hit(x1, y1, x2, y2));
        } else if i2 && let Some(q) = hit(x1, y1, x2, y2) {
            v.push(q);
            v.push((x2, y2));
        }
    }
    let area = (0..v.len()).map(|i| v[i].0 * v[(i + 1) % v.len()].1 - v[(i + 1) % v.len()].0 * v[i].1).sum::<f64>().abs() * 0.5;
    if v.len() < 3 || area < 0.5 {
        return MaskPath { vertices: Vec::new(), closed: true };
    }
    polygon(v.into_iter().map(|(x, y)| Vec2::new(x - m, y - m)).collect())
}

/// The path of one of Cut's masks in the element's own pixels (`w` x `h`), as its definition draws it.
pub(super) fn cut_mask_path(mask: &Value, w: f64, h: f64) -> Result<MaskPath, String> {
    let ty = mask.get("type").and_then(Value::as_str).unwrap_or("");
    let p = mask.get("params").ok_or("a mask without params")?;
    Ok(match ty {
        "split" => split(p, w, h),
        "cinematic-bars" => {
            let cx = w / 2.0 + num(p, "centerX", 0.0) * w;
            let cy = h / 2.0 + num(p, "centerY", 0.0) * h;
            let mw = (num(p, "width", 1.0) * w).max(w);
            let mh = num(p, "height", 0.6).max(0.01) * h;
            rect(cx, cy, mw / 2.0, mh / 2.0, num(p, "rotation", 0.0) * PI / 180.0)
        }
        "rectangle" => {
            let (cx, cy, mw, mh, rot) = box_like(p, w, h);
            rect(cx, cy, mw / 2.0, mh / 2.0, rot)
        }
        "diamond" => {
            let (cx, cy, mw, mh, rot) = box_like(p, w, h);
            let (hw, hh) = (mw / 2.0, mh / 2.0);
            polygon([(cx, cy - hh), (cx + hw, cy), (cx, cy + hh), (cx - hw, cy)].into_iter().map(|(x, y)| rotate_about(x, y, cx, cy, rot)).collect())
        }
        "star" => {
            let (cx, cy, mw, mh, rot) = box_like(p, w, h);
            let (hw, hh) = (mw / 2.0, mh / 2.0);
            polygon(
                (0..10)
                    .map(|i| {
                        let (rx, ry) = if i % 2 == 0 { (hw, hh) } else { (hw * 0.45, hh * 0.45) };
                        let a = (i as f64 * PI) / 5.0 - PI / 2.0;
                        rotate_about(cx + rx * a.cos(), cy + ry * a.sin(), cx, cy, rot)
                    })
                    .collect(),
            )
        }
        "ellipse" => {
            let (cx, cy, mw, mh, rot) = box_like(p, w, h);
            let (rx, ry) = (mw / 2.0, mh / 2.0);
            let r = |v: Vec2| Vec2::new(v.x * rot.cos() - v.y * rot.sin(), v.x * rot.sin() + v.y * rot.cos());
            let local = [((0.0, -ry), (rx * KAPPA, 0.0)), ((rx, 0.0), (0.0, ry * KAPPA)), ((0.0, ry), (-rx * KAPPA, 0.0)), ((-rx, 0.0), (0.0, -ry * KAPPA))];
            MaskPath {
                vertices: local
                    .into_iter()
                    .map(|((px, py), (tx, ty))| {
                        let (q, u) = (r(Vec2::new(px, py)), r(Vec2::new(tx, ty)));
                        MaskVertex { p: Vec2::new(cx + q.x, cy + q.y), t_in: Vec2::new(-u.x, -u.y), t_out: u }
                    })
                    .collect(),
                closed: true,
            }
        }
        "heart" => {
            let (cx, cy, mw, mh, rot) = box_like(p, w, h);
            let (hw, hh) = (mw / 2.0, mh / 2.0);
            let at = |lx: f64, ly: f64| rotate_about(cx + lx, cy + ly, cx, cy, rot);
            let start = at(0.0, -hh * 0.475);
            let (rc1, rc2, bottom) = (at(hw, -hh * 1.225), at(hw, -hh * 0.125), at(0.0, hh * 0.725));
            let (lc1, lc2) = (at(-hw, -hh * 0.125), at(-hw, -hh * 1.225));
            MaskPath { vertices: vec![MaskVertex { p: start, t_in: lc2 - start, t_out: rc1 - start }, MaskVertex { p: bottom, t_in: rc2 - bottom, t_out: lc1 - bottom }], closed: true }
        }
        "custom" => {
            // An open path is not drawn by Cut (its definition calls it inactive); Film keeps it as an open path whose mode is None.
            let (cx, cy) = (w / 2.0 + num(p, "centerX", 0.0) * w, h / 2.0 + num(p, "centerY", 0.0) * h);
            let (rot, scale) = (num(p, "rotation", 0.0) * PI / 180.0, num(p, "scale", 1.0));
            let to = |x: f64, y: f64| {
                let (sx, sy) = (x * w * scale, y * h * scale);
                Vec2::new(cx + sx * rot.cos() - sy * rot.sin(), cy + sx * rot.sin() + sy * rot.cos())
            };
            let pts = p.get("path").and_then(Value::as_array).cloned().unwrap_or_default();
            let vertices = pts
                .iter()
                .map(|q| {
                    let (x, y) = (num(q, "x", 0.0), num(q, "y", 0.0));
                    let a = to(x, y);
                    MaskVertex { p: a, t_in: to(x + num(q, "inX", 0.0), y + num(q, "inY", 0.0)) - a, t_out: to(x + num(q, "outX", 0.0), y + num(q, "outY", 0.0)) - a }
                })
                .collect();
            MaskPath { vertices, closed: p.get("closed").and_then(Value::as_bool).unwrap_or(false) }
        }
        "text" => return Err("a text mask, which Film draws once its text mask is built".into()),
        other => return Err(format!("the mask \"{other}\", which is not one of Cut's")),
    })
}

fn label(ty: &str) -> &'static str {
    match ty {
        "split" => "Split",
        "cinematic-bars" => "Cinematic Bars",
        "rectangle" => "Rectangle",
        "ellipse" => "Ellipse",
        "heart" => "Heart",
        "diamond" => "Diamond",
        "star" => "Star",
        "text" => "Text",
        _ => "Custom",
    }
}

fn mode_of(s: Option<&str>) -> Result<MaskMode, String> {
    match s {
        None => Ok(MaskMode::Add),
        Some(s) => MaskMode::from_name(s).ok_or_else(|| format!("the mask mode \"{s}\", which is not one of Film's")),
    }
}

fn mode_name(m: MaskMode) -> &'static str {
    match m {
        MaskMode::None => "none",
        MaskMode::Add => "add",
        MaskMode::Subtract => "subtract",
        MaskMode::Intersect => "intersect",
        MaskMode::Lighten => "lighten",
        MaskMode::Darken => "darken",
        MaskMode::Difference => "difference",
    }
}

fn track_of(s: Option<&str>) -> Result<TrackMethod, String> {
    match s {
        None => Ok(TrackMethod::default()),
        Some("position") => Ok(TrackMethod::Position),
        Some("position-rotation") => Ok(TrackMethod::PositionRotation),
        Some("position-scale-rotation") => Ok(TrackMethod::PositionScaleRotation),
        Some(s) => Err(format!("the tracking \"{s}\", which is not one of Film's")),
    }
}

fn track_name(t: TrackMethod) -> &'static str {
    match t {
        TrackMethod::Position => "position",
        TrackMethod::PositionRotation => "position-rotation",
        TrackMethod::PositionScaleRotation => "position-scale-rotation",
    }
}

/// One of Cut's masks as Film's, its feather and expansion in the units Cut measures them in.
pub(super) fn film_mask(mask: &Value, b: &ElementBox) -> Result<Mask, String> {
    let ty = mask.get("type").and_then(Value::as_str).unwrap_or("");
    let p = mask.get("params").ok_or("a mask without params")?;
    let mut path = cut_mask_path(mask, b.w, b.h)?;
    for v in &mut path.vertices {
        v.p = Vec2::new(v.p.x + b.x, v.p.y + b.y);
    }
    let units = match p.get("units").and_then(Value::as_str) {
        None | Some("canvas") => MaskUnits::Canvas,
        Some("clip") => MaskUnits::Clip,
        Some(u) => return Err(format!("a mask measured in \"{u}\"")),
    };
    let open = ty == "custom" && !path.closed;
    let mut m = Mask::new(mask.get("name").and_then(Value::as_str).unwrap_or(label(ty)), path);
    m.units = units;
    m.feather = Param::new(ParamValue::Float(num(p, "feather", 0.0)));
    m.expansion = Param::new(ParamValue::Float(num(p, "expansion", 0.0)));
    m.opacity = Param::new(ParamValue::Float(num(p, "opacity", 1.0) * 100.0));
    m.inverted = p.get("inverted").and_then(Value::as_bool).unwrap_or(false);
    m.mode = if open { MaskMode::None } else { mode_of(p.get("mode").and_then(Value::as_str))? };
    m.track_method = track_of(mask.get("track").and_then(Value::as_str))?;
    Ok(m)
}

/// Cut's masks as Film's.
pub(super) fn film_masks(masks: &[Value], b: &ElementBox) -> Result<Vec<Mask>, String> {
    masks.iter().map(|m| film_mask(m, b)).collect()
}

fn r9(x: f64) -> Value {
    let y = (x * 1e9).round() / 1e9;
    let y = if y == 0.0 { 0.0 } else { y };
    if y.fract() == 0.0 && y.abs() < 9e15 { json!(y as i64) } else { json!(y) }
}

fn static_f64(p: &Param, what: &str, name: &str) -> Result<f64, String> {
    if p.is_animated() {
        return Err(format!("mask-keyframes-not-in-cut: Film animates the {what} of the mask \"{name}\", and Cut's masks hold no keyframes"));
    }
    Ok(p.value.as_f64().unwrap_or(0.0))
}

/// Film's path as Cut's custom path points (fractions of the element, about its centre).
fn custom_points(path: &MaskPath, b: &ElementBox) -> Value {
    Value::Array(
        path.vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                json!({"id": format!("film-p{i}"), "x": r9((v.p.x - b.x) / b.w - 0.5), "y": r9((v.p.y - b.y) / b.h - 0.5),
                    "inX": r9(v.t_in.x / b.w), "inY": r9(v.t_in.y / b.h), "outX": r9(v.t_out.x / b.w), "outY": r9(v.t_out.y / b.h)})
            })
            .collect(),
    )
}

/// Film's masks written back in Cut's shape over `origin` (Cut's masks as the file holds them): what Film did not change comes back
/// exactly; a changed mask has only what changed rewritten; a mask Film added is Cut's custom path in its own units.
pub(super) fn cut_masks(film: &[Mask], origin: &[Value], b: &ElementBox) -> Result<Vec<Value>, String> {
    let expected: Vec<Option<Mask>> = origin.iter().map(|o| film_mask(o, b).ok()).collect();
    let mut out = Vec::with_capacity(film.len());
    for (i, m) in film.iter().enumerate() {
        let e = expected.get(i).cloned().flatten();
        if let (Some(o), Some(e)) = (origin.get(i), &e)
            && m == e
        {
            out.push(o.clone());
            continue;
        }
        if m.path.is_animated() {
            return Err(format!("mask-keyframes-not-in-cut: Film animates the path of the mask \"{}\", and Cut's masks hold no keyframes", m.name));
        }
        let ParamValue::Path(path) = &m.path.value else { return Err(format!("the mask \"{}\" has no path", m.name)) };
        let feather = static_f64(&m.feather, "feather", &m.name)?;
        let expansion = static_f64(&m.expansion, "expansion", &m.name)?;
        let opacity = static_f64(&m.opacity, "opacity", &m.name)?;
        let fresh = || json!({"id": format!("film-mask-{i}"), "type": "custom", "params": {"feather": 0, "inverted": false, "strokeColor": "#ffffff", "strokeWidth": 0, "strokeAlign": "center"}});
        let (mut o, e) = match (origin.get(i), e) {
            (Some(o), Some(e)) => (o.clone(), Some(e)),
            _ => (fresh(), None),
        };
        let mut params = o["params"].as_object().cloned().ok_or("a mask of Cut's without params")?;
        let params = &mut params;
        let reshaped = e.as_ref().is_none_or(|e| e.path.value != m.path.value);
        if reshaped {
            // Film's curve as Cut's custom path: the shape's own parameters go, its look (feather, stroke, inversion...) stays.
            let keep = ["feather", "inverted", "strokeColor", "strokeWidth", "strokeAlign", "mode", "opacity", "expansion", "units"];
            params.retain(|k, _| keep.contains(&k.as_str()));
            let closed = path.closed || m.mode != MaskMode::None;
            for (k, v) in [("path", custom_points(path, b)), ("closed", json!(closed)), ("centerX", json!(0)), ("centerY", json!(0)), ("rotation", json!(0)), ("scale", json!(1))] {
                params.insert(k.into(), v);
            }
        }
        if e.as_ref().is_none_or(|e| e.units != m.units) {
            params.insert("units".into(), json!(m.units.label()));
        }
        let measured_changed = e.as_ref().is_none_or(|e| e.feather != m.feather || e.expansion != m.expansion);
        if measured_changed {
            params.insert("feather".into(), r9(feather));
            params.insert("expansion".into(), r9(expansion));
        }
        let set_opt = |params: &mut Map<String, Value>, k: &str, v: Value, default: bool| {
            if default && !params.contains_key(k) {
                return;
            }
            params.insert(k.into(), v);
        };
        if e.as_ref().is_none_or(|e| e.opacity != m.opacity) {
            set_opt(params, "opacity", r9(opacity / 100.0), opacity == 100.0);
        }
        if e.as_ref().is_none_or(|e| e.mode != m.mode) && !(m.mode == MaskMode::None && !path.closed) {
            set_opt(params, "mode", json!(mode_name(m.mode)), m.mode == MaskMode::Add);
        }
        if e.as_ref().is_none_or(|e| e.inverted != m.inverted) {
            params.insert("inverted".into(), json!(m.inverted));
        }
        o["params"] = Value::Object(params.clone());
        if reshaped {
            o["type"] = json!("custom");
        }
        // The name Film shows is Cut's mask's name, else its type's: written where that would not give Film's back.
        let shown = o.get("name").and_then(Value::as_str).unwrap_or(label(o["type"].as_str().unwrap_or(""))).to_string();
        if shown != m.name {
            o["name"] = json!(m.name);
        }
        if e.as_ref().is_none_or(|e| e.track_method != m.track_method) && (e.is_some() || m.track_method != TrackMethod::default()) {
            o["track"] = json!(track_name(m.track_method));
        }
        out.push(o);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../../tests/it/oneos/mask-paths.json")).unwrap()
    }

    #[test]
    fn cut_s_masks_are_the_paths_the_editor_draws() {
        let f = fixture();
        let cases = f["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 32);
        for c in cases {
            let (w, h) = (c["width"].as_f64().unwrap(), c["height"].as_f64().unwrap());
            let mask = json!({"type": c["type"], "params": c["params"]});
            let path = cut_mask_path(&mask, w, h).unwrap();
            let want = c["vertices"].as_array().unwrap();
            assert_eq!(path.vertices.len(), want.len(), "{} {w}x{h}: vertex count", c["type"]);
            assert!(path.closed);
            for (v, x) in path.vertices.iter().zip(want) {
                let near = |a: f64, b: &Value| (a - b.as_f64().unwrap()).abs() < 1e-6;
                for (got, key) in [([v.p.x, v.p.y], "p"), ([v.t_in.x, v.t_in.y], "t_in"), ([v.t_out.x, v.t_out.y], "t_out")] {
                    assert!(near(got[0], &x[key][0]) && near(got[1], &x[key][1]), "{} {w}x{h} {key}: {got:?} where the editor draws {}", c["type"], x[key]);
                }
            }
        }
    }

    fn b() -> ElementBox {
        ElementBox { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }
    }

    fn masks() -> Vec<Value> {
        fixture()["cases"].as_array().unwrap().iter().filter(|c| c["width"] == 1920).enumerate().map(|(i, c)| json!({"id": format!("m{i}"), "type": c["type"], "params": c["params"]})).collect()
    }

    #[test]
    fn cut_s_masks_unchanged_in_film_are_written_back_exactly() {
        let o = masks();
        let film = film_masks(&o, &b()).unwrap();
        assert_eq!(cut_masks(&film, &o, &b()).unwrap(), o);
    }

    #[test]
    fn film_s_edit_of_a_cut_mask_is_written_in_cut_s_shape_and_reads_back_as_film_s() {
        let mut o = masks();
        o[4]["params"]["feather"] = json!(8);
        let mut film = film_masks(&o, &b()).unwrap();
        assert_eq!((film[4].units, film[4].feather.value.as_f64()), (MaskUnits::Canvas, Some(8.0)), "Cut's 8 canvas pixels are Film's");
        // Film reshapes the rectangle, sets the ellipse's mode, opacity, expansion and feather, and adds a mask of its own.
        let ParamValue::Path(p) = &mut film[4].path.value else { panic!() };
        p.vertices[0].p = Vec2::new(600.0, 200.0);
        film[6].mode = MaskMode::Subtract;
        film[6].opacity = Param::new(ParamValue::Float(50.0));
        film[6].expansion = Param::new(ParamValue::Float(3.0));
        film[6].feather = Param::new(ParamValue::Float(5.0));
        film[6].name = "Hole".into();
        film.push(Mask::new("Custom", MaskPath::ellipse(Vec2::new(300.0, 300.0), Vec2::new(50.0, 40.0))));
        let cut = cut_masks(&film, &o, &b()).unwrap();
        assert_eq!(cut[4]["type"], "custom");
        assert_eq!(cut[4]["id"], o[4]["id"]);
        assert_eq!(cut[4]["params"]["feather"], 8, "the feather Film did not change stays in the canvas's pixels");
        assert!(cut[4]["params"].get("width").is_none(), "the rectangle's own parameters go with its shape");
        assert_eq!(cut[6]["type"], "ellipse", "a mask Film did not reshape keeps its type");
        assert_eq!(cut[6]["params"]["mode"], "subtract");
        assert_eq!(cut[6]["params"]["opacity"], 0.5);
        assert_eq!((cut[6]["params"].get("units"), &cut[6]["params"]["expansion"], &cut[6]["params"]["feather"]), (None, &json!(3), &json!(5)), "Film's feather in the canvas's pixels, Cut's own");
        assert_eq!(cut[6]["name"], "Hole");
        assert_eq!(cut[16]["type"], "custom");
        assert_eq!(cut[16]["params"]["units"], "clip");
        assert_eq!(cut.len(), 17);
        for (i, x) in cut.iter().enumerate() {
            if ![4, 6, 16].contains(&i) {
                assert_eq!(x, &o[i], "mask {i} Film did not change");
            }
        }
        // What Film wrote opens in Film as what Film had (the same curve, the same look).
        let again = film_masks(&cut, &b()).unwrap();
        for (a, f) in again.iter().zip(&film) {
            let (ParamValue::Path(pa), ParamValue::Path(pf)) = (&a.path.value, &f.path.value) else { panic!() };
            assert_eq!(pa.vertices.len(), pf.vertices.len());
            for (x, y) in pa.vertices.iter().zip(&pf.vertices) {
                assert!((x.p - y.p).length() < 1e-6 && (x.t_in - y.t_in).length() < 1e-6 && (x.t_out - y.t_out).length() < 1e-6, "{}: {x:?} {y:?}", a.name);
            }
            assert_eq!((a.mode, a.inverted, &a.name, a.track_method, a.units), (f.mode, f.inverted, &f.name, f.track_method, f.units));
            for (p, q) in [(&a.feather, &f.feather), (&a.expansion, &f.expansion), (&a.opacity, &f.opacity)] {
                assert!((p.value.as_f64().unwrap() - q.value.as_f64().unwrap()).abs() < 1e-9, "{}", a.name);
            }
        }
    }

    #[test]
    fn what_cut_cannot_hold_of_a_film_mask_is_refused_by_name() {
        let o = masks();
        let mut film = film_masks(&o, &b()).unwrap();
        let mut k = film[2].feather.clone();
        k.keyframes.push(filmcraft_project::Keyframe::new(filmcraft_time::Tick(0), ParamValue::Float(2.0)));
        film[2].feather = k;
        assert!(cut_masks(&film, &o, &b()).unwrap_err().starts_with("mask-keyframes-not-in-cut: "));
        let mut odd = o.clone();
        odd[2]["params"]["units"] = json!("inches");
        assert_eq!(film_masks(&odd, &b()).unwrap_err(), "a mask measured in \"inches\"");
    }

    /// files/989 (Mona 2026-11-09: "Build canvas feather in Film"): a feather and expansion Cut measures in the canvas's pixels open in
    /// Film as its canvas mask with the same numbers, needing no scale; one in clip pixels as its clip mask. A person switching a mask's
    /// units in Film writes the units Cut reads; Film's edit of a canvas feather stays in the canvas's pixels.
    #[test]
    fn a_canvas_feather_is_film_s_canvas_feather_both_ways() {
        let mut o = masks();
        o[2]["params"]["feather"] = json!(4);
        o[2]["params"]["expansion"] = json!(-1.5);
        o[3]["params"]["units"] = json!("clip");
        o[3]["params"]["feather"] = json!(6);
        let mut film = film_masks(&o, &b()).unwrap();
        let measures = |m: &Mask| (m.units, m.feather.value.as_f64(), m.expansion.value.as_f64());
        assert_eq!(measures(&film[2]), (MaskUnits::Canvas, Some(4.0), Some(-1.5)));
        assert_eq!(measures(&film[3]), (MaskUnits::Clip, Some(6.0), Some(0.0)));
        assert_eq!(cut_masks(&film, &o, &b()).unwrap(), o, "unchanged, written back exactly");
        film[2].feather = Param::new(ParamValue::Float(9.0));
        film[3].units = MaskUnits::Canvas;
        film[5].units = MaskUnits::Clip;
        let cut = cut_masks(&film, &o, &b()).unwrap();
        assert_eq!((cut[2]["params"].get("units"), &cut[2]["params"]["feather"]), (None, &json!(9)), "Film's edit of a canvas feather");
        assert_eq!((&cut[3]["params"]["units"], &cut[3]["params"]["feather"]), (&json!("canvas"), &json!(6)));
        assert_eq!(&cut[5]["params"]["units"], &json!("clip"));
        assert_eq!(film_masks(&cut, &b()).unwrap().iter().map(|m| m.units).collect::<Vec<_>>(), film.iter().map(|m| m.units).collect::<Vec<_>>());
    }
}
