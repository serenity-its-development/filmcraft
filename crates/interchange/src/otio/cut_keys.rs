//! Keyframes as both tools hold them (OneOS doctrine files/989, step 2; Mona 2026-11-09: "yes, so this becomes a tool choice not a file
//! type choicer"). The same bridge as the OpenCut fork's `apps/web/src/oneos/keys.ts`, from Film's side.
//!
//! Cut animates a value as channels of scalar keys (OpenCut `animation/types.ts`: a key's time is Cut's ticks from the element's start; its
//! segment to the next is `step`, `linear` or `bezier` with time/value handles `{dt, dv}`, a third of the segment when unset). Film animates
//! a parameter as keyframes ([`filmcraft_project::Keyframe`]: Film's ticks from the clip's start; the segment from key a to key b is held
//! when a is `Hold`, else its out side is eased with a flat handle reaching `a.out_influence` when a is Bezier, Auto Bezier, Continuous
//! Bezier or Ease Out, and its in side, reaching `b.in_influence`, when b is Bezier, Auto Bezier, Continuous Bezier or Ease In or a is
//! Bezier or Continuous Bezier; a side not eased is a straight third). An eased side may be SLOPED (Mona 2026-11-09: "Add slopes to
//! Film"): the key then holds its handle's rise per component (`out_rise`, `in_rise`, in Film's units) and its reach is 0% to 100% of the
//! segment. So every Film segment IS a Cut segment and every Cut segment is a Film one (a side is a straight third, a flat handle or a
//! sloped one); what Film still cannot hold is a value's components keyed at different times or curving differently ([`cut_to_film`]
//! finds each key's interp, keeping the one it had where it fits).

use filmcraft_project::{Interpolation, Keyframe, ParamValue};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

/// Cut's clock: ticks per second.
const CUT_TICKS: i64 = 120_000;
const PER_CUT_TICK: i64 = TICKS_PER_SECOND / CUT_TICKS;
const THIRD: f64 = 1.0 / 3.0;
const ORDER: [Interpolation; 7] = [
    Interpolation::Linear,
    Interpolation::Hold,
    Interpolation::EaseOut,
    Interpolation::EaseIn,
    Interpolation::AutoBezier,
    Interpolation::Bezier,
    Interpolation::ContinuousBezier,
];

fn out_eased(i: Interpolation) -> bool {
    matches!(i, Interpolation::Bezier | Interpolation::AutoBezier | Interpolation::ContinuousBezier | Interpolation::EaseOut)
}
fn in_eased_by_next(i: Interpolation) -> bool {
    matches!(i, Interpolation::Bezier | Interpolation::AutoBezier | Interpolation::ContinuousBezier | Interpolation::EaseIn)
}
fn in_eased_by_self(i: Interpolation) -> bool {
    matches!(i, Interpolation::Bezier | Interpolation::ContinuousBezier)
}
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}
fn clamp_influence(x: f64) -> f64 {
    x.clamp(0.01, 1.0)
}
/// A sloped handle's reach: 0% to 100% of the segment (Cut clamps a handle's dt to the segment).
fn reach(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}
fn number(x: f64) -> Value {
    let r = (x * 1e9).round() / 1e9;
    if r.fract() == 0.0 && r.abs() < 9e15 { json!(r as i64) } else { json!(r) }
}

/// One of Cut's scalar keys.
#[derive(Debug, Clone, PartialEq)]
pub struct CutKey {
    pub time: i64,
    pub value: f64,
    pub segment: String,
    /// Handles as `(dt, dv)`: the right one leaves this key, the left one enters it.
    pub right: Option<(f64, f64)>,
    pub left: Option<(f64, f64)>,
    /// What else the key holds (its id, tangent mode), kept as the file held it.
    pub rest: serde_json::Map<String, Value>,
}

impl CutKey {
    /// A key of Cut's channel as the file holds it.
    pub fn read(v: &Value) -> Option<CutKey> {
        let o = v.as_object()?;
        let handle = |k: &str| o.get(k).and_then(|h| Some((h.get("dt")?.as_f64()?, h.get("dv")?.as_f64()?)));
        let mut rest = o.clone();
        for k in ["time", "value", "segmentToNext", "leftHandle", "rightHandle"] {
            rest.remove(k);
        }
        Some(CutKey {
            time: o.get("time")?.as_i64()?,
            value: o.get("value")?.as_f64()?,
            segment: o.get("segmentToNext").and_then(Value::as_str).unwrap_or("linear").to_string(),
            right: handle("rightHandle"),
            left: handle("leftHandle"),
            rest,
        })
    }

    /// The key in Cut's shape.
    pub fn write(&self) -> Value {
        let mut o = self.rest.clone();
        o.insert("time".into(), json!(self.time));
        o.insert("value".into(), number(self.value));
        o.insert("segmentToNext".into(), json!(self.segment));
        if let Some((dt, dv)) = self.right {
            o.insert("rightHandle".into(), json!({"dt": number(dt), "dv": number(dv)}));
        }
        if let Some((dt, dv)) = self.left {
            o.insert("leftHandle".into(), json!({"dt": number(dt), "dv": number(dv)}));
        }
        Value::Object(o)
    }
}

/// A Film time (ticks from the clip's start) in Cut's ticks, when it is a whole one.
pub fn cut_time(t: Tick) -> Option<i64> {
    (t.0 % PER_CUT_TICK == 0).then_some(t.0 / PER_CUT_TICK)
}

/// Film's keyframes of one parameter as one channel of Cut's per component (`parts` gives a value's components in Cut's units; `per_cut[c]`
/// is Film's units in one of Cut's for component c, so a sloped handle's rise is read in Cut's units). `prior` are the channels the file
/// held: a key keeps its id and tangent mode where it stays at its time.
pub fn film_to_cut(keys: &[Keyframe], parts: &dyn Fn(&ParamValue) -> Vec<f64>, prior: &[Vec<CutKey>], id_base: &str, per_cut: &[f64]) -> Result<Vec<Vec<CutKey>>, String> {
    let values: Vec<Vec<f64>> = keys.iter().map(|k| parts(&k.value)).collect();
    let n = values.first().map(Vec::len).unwrap_or(0);
    if !keys.is_empty() && per_cut.len() != n {
        return Err(format!("its {n} components have {} unit factors", per_cut.len()));
    }
    let mut times = Vec::new();
    for k in keys {
        times.push(cut_time(k.time).ok_or_else(|| format!("a keyframe at Film tick {} is not a whole tick of Cut's clock ({CUT_TICKS}/s)", k.time.0))?);
    }
    let mut out = Vec::new();
    for c in 0..n {
        let was = prior.get(c).map(Vec::as_slice).unwrap_or(&[]);
        let mut ch: Vec<CutKey> = times
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let at = was.iter().find(|w| w.time == *t);
                let mut rest = serde_json::Map::new();
                rest.insert("id".into(), at.and_then(|w| w.rest.get("id").cloned()).unwrap_or_else(|| json!(format!("{id_base}-{c}-{i}"))));
                rest.insert("tangentMode".into(), at.and_then(|w| w.rest.get("tangentMode").cloned()).unwrap_or_else(|| json!("flat")));
                CutKey { time: *t, value: values[i][c], segment: "linear".into(), right: None, left: None, rest }
            })
            .collect();
        for i in 0..keys.len().saturating_sub(1) {
            let (a, b) = (&keys[i], &keys[i + 1]);
            if a.interp == Interpolation::Hold || !a.value.interpolates() {
                ch[i].segment = "step".into();
                continue;
            }
            let o = out_eased(a.interp);
            let m = in_eased_by_next(b.interp) || in_eased_by_self(a.interp);
            if !o && !m {
                continue;
            }
            let span = (ch[i + 1].time - ch[i].time) as f64;
            ch[i].segment = "bezier".into();
            // A side Film does not ease is Cut's default handle, left unset so it follows the keys as they move, as Film's does.
            let k = per_cut[c];
            if o {
                ch[i].right = Some(match &a.out_rise {
                    Some(r) => (reach(a.out_influence) * span, r.get(c).copied().unwrap_or(0.0) / k),
                    None => (clamp_influence(a.out_influence) * span, 0.0),
                });
            }
            if m {
                ch[i + 1].left = Some(match &b.in_rise {
                    Some(r) => (-reach(b.in_influence) * span, r.get(c).copied().unwrap_or(0.0) / k),
                    None => (-clamp_influence(b.in_influence) * span, 0.0),
                });
            }
        }
        if let Some(last) = ch.last_mut()
            && let Some(w) = was.iter().find(|w| w.time == last.time)
        {
            last.segment = w.segment.clone();
        }
        out.push(ch);
    }
    Ok(out)
}

/// One side of a Cut segment as Film can hold it: a straight third (`lin`), a flat handle reaching `flat` of the segment, and always a
/// sloped handle reaching `r` of it with a rise of `dv` per component (in Cut's units).
#[derive(Debug, Clone)]
struct Side {
    lin: bool,
    flat: Option<f64>,
    r: f64,
    dv: Vec<f64>,
}

#[derive(Debug, Clone)]
enum Seg {
    Step,
    Move { out: Side, inn: Side },
}

fn side((dt, dv): (f64, f64), span: f64, delta: f64, sign: f64) -> Side {
    let r = reach(sign * dt / span);
    let lin = near(dt, sign * span * THIRD) && near(dv, sign * delta * THIRD);
    let flat = (dv.abs() <= 1e-12 && r >= 0.01 - 1e-12).then(|| r.clamp(0.01, 1.0));
    Side { lin, flat, r, dv: vec![dv] }
}

fn segment(a: &CutKey, b: &CutKey) -> Seg {
    if a.segment == "step" {
        return Seg::Step;
    }
    let (span, delta) = ((b.time - a.time) as f64, b.value - a.value);
    if a.segment == "linear" || span == 0.0 {
        let flat = (delta == 0.0).then_some(THIRD);
        return Seg::Move { out: Side { lin: true, flat, r: THIRD, dv: vec![delta * THIRD] }, inn: Side { lin: true, flat, r: THIRD, dv: vec![-delta * THIRD] } };
    }
    let out = side(a.right.unwrap_or((span * THIRD, delta * THIRD)), span, delta, 1.0);
    let inn = side(b.left.unwrap_or((-span * THIRD, -delta * THIRD)), span, delta, -1.0);
    Seg::Move { out, inn }
}

/// Two components' sides as one side of Film's (Film eases a value's components together), or None when they reach differently.
fn both(x: &Side, y: &Side) -> Option<Side> {
    if !near(x.r, y.r) {
        return None;
    }
    let flat = match (x.flat, y.flat) {
        (Some(a), Some(b)) if near(a, b) => Some(a),
        _ => None,
    };
    Some(Side { lin: x.lin && y.lin, flat, r: x.r, dv: x.dv.iter().chain(&y.dv).copied().collect() })
}

fn side_same(x: &Side, y: &Side) -> bool {
    (x.lin && y.lin) || matches!((x.flat, y.flat), (Some(u), Some(v)) if near(u, v)) || (near(x.r, y.r) && x.dv.len() == y.dv.len() && x.dv.iter().zip(&y.dv).all(|(a, b)| near(*a, *b)))
}

/// Cut's channels for one of Film's parameters (one per component) as Film's keyframes: `value` makes Film's value from the components,
/// `prior` are the keyframes Film held (a key keeps its interp where it still fits), `per_cut[c]` Film's units in one of Cut's for
/// component c (a sloped handle's rise is written in Film's units). A reason when Film cannot hold the curve.
pub fn cut_to_film(channels: &[Vec<CutKey>], value: &dyn Fn(&[f64]) -> ParamValue, prior: &[Keyframe], per_cut: &[f64]) -> Result<Vec<Keyframe>, String> {
    let Some(first) = channels.first() else { return Ok(Vec::new()) };
    if per_cut.len() != channels.len() {
        return Err(format!("its {} components have {} unit factors", channels.len(), per_cut.len()));
    }
    if channels.iter().any(|c| c.len() != first.len() || c.iter().zip(first).any(|(k, f)| k.time != f.time)) {
        return Err("its components are keyed at different times, where Film keys them together".into());
    }
    let n = first.len();
    let mut segs = Vec::new();
    for i in 0..n.saturating_sub(1) {
        let mut s: Option<Seg> = None;
        for ch in channels {
            let x = segment(&ch[i], &ch[i + 1]);
            s = Some(match (s, x) {
                (None, x) => x,
                (Some(Seg::Step), Seg::Step) => Seg::Step,
                (Some(Seg::Move { out: a, inn: b }), Seg::Move { out: c, inn: d }) => match (both(&a, &c), both(&b, &d)) {
                    (Some(out), Some(inn)) => Seg::Move { out, inn },
                    _ => return Err(format!("its components curve differently from {}, where Film eases them together", ch[i].time)),
                },
                _ => return Err(format!("its components step and move differently from {}", ch[i].time)),
            });
        }
        segs.push(s.unwrap_or(Seg::Step));
    }
    let fits = |prev: Option<Interpolation>, cur: Interpolation, i: usize| -> bool {
        if i + 1 < n {
            match &segs[i] {
                Seg::Step if cur != Interpolation::Hold => return false,
                Seg::Move { .. } if cur == Interpolation::Hold => return false,
                // An eased side is flat or sloped, so it always fits; a side not eased must be a straight third.
                Seg::Move { out, .. } => {
                    if !out_eased(cur) && !out.lin {
                        return false;
                    }
                }
                Seg::Step => {}
            }
        }
        if i > 0
            && let (Some(prev), Seg::Move { inn, .. }) = (prev, &segs[i - 1])
        {
            let eased = in_eased_by_next(cur) || in_eased_by_self(prev);
            if !eased && !inn.lin {
                return false;
            }
        }
        true
    };
    let prior_at = |t: i64| prior.iter().find(|k| cut_time(k.time) == Some(t));
    fn choose(i: usize, prev: Option<Interpolation>, n: usize, order: &dyn Fn(usize) -> Vec<Interpolation>, fits: &dyn Fn(Option<Interpolation>, Interpolation, usize) -> bool) -> Option<Vec<Interpolation>> {
        if i == n {
            return Some(Vec::new());
        }
        for cur in order(i) {
            if !fits(prev, cur, i) {
                continue;
            }
            if let Some(mut rest) = choose(i + 1, Some(cur), n, order, fits) {
                rest.insert(0, cur);
                return Some(rest);
            }
        }
        None
    }
    let order = |i: usize| -> Vec<Interpolation> {
        match prior_at(first[i].time).map(|k| k.interp) {
            Some(was) => std::iter::once(was).chain(ORDER.into_iter().filter(|x| *x != was)).collect(),
            None => ORDER.to_vec(),
        }
    };
    let interps = choose(0, None, n, &order, &fits).ok_or("its curve's sides do not pair up as Film's keyframes ease them")?;
    Ok(first
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let was = prior_at(k.time);
            let out_side = match segs.get(i) {
                Some(Seg::Move { out, .. }) if i + 1 < n && out_eased(interps[i]) => Some(out),
                _ => None,
            };
            let in_side = match i.checked_sub(1).map(|j| &segs[j]) {
                Some(Seg::Move { inn, .. }) if in_eased_by_next(interps[i]) || in_eased_by_self(interps[i - 1]) => Some(inn),
                _ => None,
            };
            // A side Film eases flat holds no rise; a sloped one holds its rise per component, in Film's units.
            let rise = |x: &Side| x.dv.iter().zip(per_cut).map(|(d, k)| (d * k * 1e9).round() / 1e9).collect::<Vec<f64>>();
            let comps: Vec<f64> = channels.iter().map(|c| c[i].value).collect();
            Keyframe {
                time: Tick(k.time * PER_CUT_TICK),
                value: value(&comps),
                interp: interps[i],
                out_influence: out_side.map(|x| x.flat.unwrap_or(x.r)).or(was.map(|w| w.out_influence)).unwrap_or(THIRD),
                in_influence: in_side.map(|x| x.flat.unwrap_or(x.r)).or(was.map(|w| w.in_influence)).unwrap_or(THIRD),
                out_rise: out_side.filter(|x| x.flat.is_none()).map(rise),
                in_rise: in_side.filter(|x| x.flat.is_none()).map(rise),
            }
        })
        .collect())
}

/// Two of Cut's channels hold the same curve (ids, tangent modes and handles a segment does not use aside).
pub fn same_curve(a: &[CutKey], b: &[CutKey]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if a[i].time != b[i].time || !near(a[i].value, b[i].value) {
            return false;
        }
        if i + 1 == a.len() {
            break;
        }
        match (segment(&a[i], &a[i + 1]), segment(&b[i], &b[i + 1])) {
            (Seg::Step, Seg::Step) => {}
            (Seg::Move { out: p, inn: q }, Seg::Move { out: r, inn: s }) => {
                if !(side_same(&p, &r) && side_same(&q, &s)) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: i64, v: f64, interp: Interpolation, o: f64, i: f64) -> Keyframe {
        Keyframe { time: Tick(t * PER_CUT_TICK), value: ParamValue::Float(v), interp, out_influence: o, in_influence: i, out_rise: None, in_rise: None }
    }
    fn parts(v: &ParamValue) -> Vec<f64> {
        vec![v.as_f64().unwrap_or(0.0)]
    }
    fn value(c: &[f64]) -> ParamValue {
        ParamValue::Float(c[0])
    }

    /// Every combination of Film's interps on three keys reads into Cut and back as the same curve.
    #[test]
    fn every_film_curve_is_a_cut_curve_and_back() {
        for a in ORDER {
            for b in ORDER {
                for c in ORDER {
                    let film = vec![key(0, 0.0, a, 0.25, 0.5), key(1200, 10.0, b, 0.75, 0.2), key(3000, -5.0, c, THIRD, 0.6)];
                    let cut = film_to_cut(&film, &parts, &[], "k", &[1.0]).unwrap();
                    let back = cut_to_film(&cut, &value, &film, &[1.0]).unwrap_or_else(|e| panic!("{a:?} {b:?} {c:?}: {e}"));
                    let again = film_to_cut(&back, &parts, &cut, "k", &[1.0]).unwrap();
                    assert!(same_curve(&cut[0], &again[0]), "{a:?} {b:?} {c:?}");
                    for t in (0..3000).step_by(37) {
                        let p = filmcraft_project::Param { value: ParamValue::Float(0.0), keyframes: film.clone() };
                        let q = filmcraft_project::Param { value: ParamValue::Float(0.0), keyframes: back.clone() };
                        assert!(near(p.f64_at(Tick(t * PER_CUT_TICK)), q.f64_at(Tick(t * PER_CUT_TICK))), "{a:?} {b:?} {c:?} at {t}");
                    }
                }
            }
        }
    }

    /// Mona 2026-11-09 (files/989): "Add slopes to Film". Cut's sloped handles (any reach 0% to 100%, any rise) are Film's sloped keys,
    /// drawn as Cut draws them, and read back as the very handles; a flat handle stays a flat ease with no rise.
    #[test]
    fn a_handle_with_a_slope_is_held_by_film_and_draws_as_cut_draws_it() {
        let k0 = CutKey::read(&json!({"id": "a", "time": 0, "value": 0, "segmentToNext": "bezier", "tangentMode": "broken", "rightHandle": {"dt": 100, "dv": 4}})).unwrap();
        let k1 = CutKey::read(&json!({"id": "b", "time": 600, "value": 10, "segmentToNext": "bezier", "tangentMode": "broken", "leftHandle": {"dt": 0, "dv": 3}})).unwrap();
        let k2 = CutKey::read(&json!({"id": "c", "time": 900, "value": 10, "segmentToNext": "linear", "tangentMode": "flat"})).unwrap();
        let cut = vec![k0.clone(), k1.clone(), k2];
        // Film's units are twice Cut's here: the rise is written in Film's units.
        let f = cut_to_film(std::slice::from_ref(&cut), &|c| ParamValue::Float(c[0] * 2.0), &[], &[2.0]).unwrap();
        assert_eq!((f[0].out_rise.clone(), f[0].out_influence), (Some(vec![8.0]), 100.0 / 600.0));
        assert_eq!((f[1].in_rise.clone(), f[1].in_influence), (Some(vec![6.0]), 0.0), "a zero-reach handle is held, not clamped to 1%");
        let back = film_to_cut(&f, &|v| vec![v.as_f64().unwrap() / 2.0], std::slice::from_ref(&cut), "k", &[2.0]).unwrap();
        assert_eq!((back[0][0].right, back[0][1].left), (Some((100.0, 4.0)), Some((-0.0, 3.0))), "Cut reads the very handles back");
        assert!(same_curve(&cut, &back[0]));
        // Film draws it as Cut does: Cut's bezier solved for time (OpenCut `animation/bezier.ts`), against Film's evaluation.
        let p = filmcraft_project::Param { value: ParamValue::Float(0.0), keyframes: f };
        let cut_at = |t: f64| {
            let (p0, p1, p2, p3) = ((0.0, 0.0), (100.0, 4.0), (600.0, 13.0), (600.0, 10.0));
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            let bz = |s: f64, a: f64, b: f64, c: f64, d: f64| (1.0 - s).powi(3) * a + 3.0 * (1.0 - s).powi(2) * s * b + 3.0 * (1.0 - s) * s * s * c + s.powi(3) * d;
            for _ in 0..80 {
                let m = (lo + hi) / 2.0;
                if bz(m, p0.0, p1.0, p2.0, p3.0) < t { lo = m } else { hi = m }
            }
            bz((lo + hi) / 2.0, p0.1, p1.1, p2.1, p3.1)
        };
        for t in (10..600).step_by(10) {
            let film = p.f64_at(Tick(t * PER_CUT_TICK)) / 2.0;
            assert!((film - cut_at(t as f64)).abs() < 1e-6, "at {t}: Film {film}, Cut {}", cut_at(t as f64));
        }
        // A flat handle is Film's flat ease, no rise.
        let mut flat = k0;
        flat.right = Some((100.0, 0.0));
        let f = cut_to_film(&[vec![flat, CutKey { segment: "linear".into(), left: None, ..k1 }]], &value, &[], &[1.0]).unwrap();
        assert_eq!((f[0].interp, f[0].out_rise.clone()), (Interpolation::EaseOut, None));
        assert!(near(f[0].out_influence, 100.0 / 600.0));
    }
}
