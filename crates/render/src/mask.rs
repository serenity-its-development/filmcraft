//! Mask coverage (CPU reference; `filmcraft_gpu::GpuMask` runs the same math in WGSL).
//!
//! Each [`Mask`] is flattened to a polygon in working-image pixels (chord error ≤ 0.05 px). For
//! a pixel centre `p` the signed distance `sd` to the polygon (positive inside, nonzero winding)
//! gives the coverage
//!
//! ```text
//! s  = sd + expansion
//! w  = max(feather, 1)
//! u  = clamp(s / w + 0.5, 0, 1)
//! c  = lerp(u, smoothstep(u), min(feather, 1))
//! m  = opacity · (inverted ? 1 − c : c)
//! ```
//!
//! so a hard mask (feather 0) is the analytic box-filtered coverage of the edge (a one-pixel
//! linear ramp across the boundary: exact for straight edges, antialiased everywhere), and a
//! feathered mask falls off smoothly over `feather` pixels centred on the (expanded) edge — a true
//! distance falloff, so corners round off and the width is the same along the whole contour.
//! A mask's distances, feather and expansion are measured in its [`MaskUnits`]: clip pixels (scaled to working pixels), or the
//! canvas's pixels (the sequence frame's), where the distance from a pixel to the edge is taken through the clip's own linear map
//! from working pixels to sequence pixels at that frame, so the feather keeps one width on screen through a keyed, uneven or rotated
//! scale (OneOS doctrine files/989, Mona 2026-11-09: "Build canvas feather in Film"). A hard canvas mask ramps over one canvas pixel.
//! Masks combine top to bottom with their [`MaskMode`]. All of this runs in linear light on the
//! premultiplied working image: a masked effect is `lerp(original, effected, coverage)` per
//! channel, an opacity mask scales the layer's premultiplied pixels.

use filmcraft_geom::{Affine, Vec2};
use filmcraft_project::{EffectInstance, Mask, MaskMode, MaskText, MaskUnits, TextDecoration};
use filmcraft_time::Tick;
use rayon::prelude::*;

use crate::image::Image;

/// Flattening tolerance in working pixels.
pub const FLATTEN_TOLERANCE: f64 = 0.05;

/// One mask ready for rasterizing, in working-image pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatMask {
    /// Closed polygons (each one's last vertex connects to its first), filled together by one non-zero winding: one for a path mask,
    /// every glyph's contours (counters included) for a text mask. Each has at least three points.
    pub contours: Vec<Vec<[f32; 2]>>,
    pub feather: f32,
    pub expansion: f32,
    /// 0..1.
    pub opacity: f32,
    pub inverted: bool,
    pub mode: MaskMode,
    /// The linear map `[a, b, c, d]` ((x, y) → (a x + c y, b x + d y)) from working pixels to the pixels `feather`, `expansion` and
    /// the distance to the edge are measured in: the identity for a clip-pixel mask (already in working pixels), the clip's map to
    /// the sequence frame for a canvas-pixel mask.
    pub metric: [f32; 4],
}

const IDENTITY_METRIC: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

/// Whether a mask is drawn: its mode is not None, and a text mask holds text (Cut's text mask with blank text is inactive and not
/// drawn, OpenCut `masks/draw-plan.ts`).
fn is_drawn(m: &Mask) -> bool {
    m.mode != MaskMode::None && m.text.as_ref().is_none_or(|t| !t.content.trim().is_empty())
}

/// A mask's shape as closed polygons in the pixels `clip_to` maps clip pixels to: its path flattened (`tol` in clip pixels), or a
/// text mask's glyphs ([`text_contours`]); `clip_to_seq` is the linear part of the clip's map to the sequence frame at `t`.
fn contours(m: &Mask, t: Tick, clip_to: &Affine, clip_to_seq: [f64; 4], tol: f64) -> Vec<Vec<[f32; 2]>> {
    let mut out = match &m.text {
        Some(text) => text_contours(text, m.units, clip_to, clip_to_seq),
        None => vec![m.path_at(t).flatten(tol).into_iter().map(|p| clip_to.apply(p)).map(|q| [q.x as f32, q.y as f32]).collect()],
    };
    out.retain(|c| c.len() >= 3);
    out
}

/// Flatten the active masks (mode ≠ None) at clip time `t`. `px_scale` = working pixels per clip pixel; `to_canvas` maps working
/// pixels to the sequence frame's pixels at `t` (its linear part measures the masks in canvas pixels).
pub fn prepare(masks: &[Mask], t: Tick, px_scale: f32, to_canvas: &Affine) -> Vec<FlatMask> {
    let s = px_scale as f64;
    let clip_to = Affine::scale(s, s);
    let clip_to_seq = [to_canvas.a * s, to_canvas.b * s, to_canvas.c * s, to_canvas.d * s];
    masks
        .iter()
        .filter(|m| is_drawn(m))
        .map(|m| {
            let (k, metric) = match m.units {
                MaskUnits::Clip => (s, IDENTITY_METRIC),
                MaskUnits::Canvas => (1.0, [to_canvas.a as f32, to_canvas.b as f32, to_canvas.c as f32, to_canvas.d as f32]),
            };
            FlatMask {
                contours: contours(m, t, &clip_to, clip_to_seq, FLATTEN_TOLERANCE / s.max(1e-6)),
                feather: (m.feather.f64_at(t).max(0.0) * k) as f32,
                expansion: (m.expansion.f64_at(t) * k) as f32,
                opacity: (m.opacity.f64_at(t) / 100.0).clamp(0.0, 1.0) as f32,
                inverted: m.inverted,
                mode: m.mode,
                metric,
            }
        })
        .collect()
}

/// Flatten the active masks at clip time `t` onto the placed layer's grid (output pixels), for a clip whose masks are drawn after its
/// Motion: `to_output` maps clip pixels to output pixels and `out_scale` is output pixels per sequence pixel. A canvas mask is measured
/// in the sequence frame's pixels, a clip mask in clip pixels (through the inverse of `to_output`), so masks of both units combine top
/// to bottom on one grid. `None` when the clip is scaled to nothing (it draws nothing).
pub fn prepare_placed(masks: &[Mask], t: Tick, to_output: &Affine, out_scale: f32) -> Option<Vec<FlatMask>> {
    let back = to_output.inverse()?;
    let stretch = least_stretch(&[to_output.a as f32, to_output.b as f32, to_output.c as f32, to_output.d as f32]).max(1e-6) as f64;
    let most = {
        let m = [to_output.a, to_output.b, to_output.c, to_output.d];
        let t2: f64 = m.iter().map(|v| v * v).sum();
        let det = (m[0] * m[3] - m[1] * m[2]).abs();
        ((t2 + (t2 * t2 - 4.0 * det * det).max(0.0).sqrt()) / 2.0).sqrt().max(stretch)
    };
    let o = 1.0 / out_scale as f64;
    let clip_to_seq = [to_output.a * o, to_output.b * o, to_output.c * o, to_output.d * o];
    Some(
        masks
            .iter()
            .filter(|m| is_drawn(m))
            .map(|m| {
                let metric = match m.units {
                    MaskUnits::Canvas => [o as f32, 0.0, 0.0, o as f32],
                    MaskUnits::Clip => [back.a as f32, back.b as f32, back.c as f32, back.d as f32],
                };
                FlatMask {
                    contours: contours(m, t, to_output, clip_to_seq, FLATTEN_TOLERANCE / most),
                    feather: m.feather.f64_at(t).max(0.0) as f32,
                    expansion: m.expansion.f64_at(t) as f32,
                    opacity: (m.opacity.f64_at(t) / 100.0).clamp(0.0, 1.0) as f32,
                    inverted: m.inverted,
                    mode: m.mode,
                    metric,
                }
            })
            .collect(),
    )
}

/// Cut's decoration stroke, in font sizes (OpenCut `text/layout.ts` `TEXT_DECORATION_THICKNESS_RATIO`), at least one canvas pixel.
const DECORATION_THICKNESS: f64 = 0.07;
/// How far above a line's middle Cut strikes it, in its ink's ascent less its descent (`STRIKETHROUGH_VERTICAL_RATIO`).
const STRIKE_RISE: f64 = 0.35;

/// The size of the canvas a text mask is drawn on (each side rounded to whole pixels, as Cut's mask canvas is): the box in clip
/// pixels, or, in the canvas's pixels, the box as the clip's linear map to the sequence frame (`clip_to_seq`) stretches each of its
/// sides.
pub fn text_canvas(t: &MaskText, units: MaskUnits, clip_to_seq: [f64; 4]) -> (f64, f64) {
    let [_, _, bw, bh] = t.frame;
    let (dw, dh) = match units {
        MaskUnits::Clip => (bw, bh),
        MaskUnits::Canvas => (bw * clip_to_seq[0].hypot(clip_to_seq[1]), bh * clip_to_seq[2].hypot(clip_to_seq[3])),
    };
    (dw.round(), dh.round())
}

/// The polygon `pts` wound with a positive signed area (y down: clockwise on screen).
fn wound(mut pts: Vec<[f32; 2]>, positive: bool) -> Vec<[f32; 2]> {
    if positive != (area(&pts) >= 0.0) {
        pts.reverse();
    }
    pts
}

fn area(pts: &[[f32; 2]]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| (pts[i][0] as f64) * (pts[(i + 1) % n][1] as f64) - (pts[(i + 1) % n][0] as f64) * (pts[i][1] as f64)).sum::<f64>() * 0.5
}

/// A text mask's outline ([`MaskText`]): every contour of every glyph of every line, and each line's decoration, mapped by `clip_to`
/// from clip pixels. Laid out by FilmCraft's text engine (shaping, the font's own outlines) as Cut lays out its text mask (OpenCut
/// `masks/definitions/text.ts`, `text/primitives.ts`): the font `size · height / 90` pixels of the canvas ([`text_canvas`]), the lines
/// `line_height` font sizes apart with the block's middle on the text's centre, each line centred on its whole advance (trailing spaces
/// and the spacing after its last character included) and set on the middle of the font's em box (a browser's `textBaseline =
/// "middle"`), then scaled and turned about the centre and the canvas laid on the box. A glyph's outer contour and a decoration are wound
/// alike, so where they meet they fill as one shape and a glyph's counters stay holes. Nothing when the canvas or the font is no pixels.
pub fn text_contours(t: &MaskText, units: MaskUnits, clip_to: &Affine, clip_to_seq: [f64; 4]) -> Vec<Vec<[f32; 2]>> {
    use filmcraft_text::{Align, ParagraphStyle, TextStyle, raster};
    let (w, h) = text_canvas(t, units, clip_to_seq);
    let px = t.size * h / 90.0;
    if w < 1.0 || h < 1.0 || !px.is_finite() || px <= 0.0 {
        return Vec::new();
    }
    let [bx, by, bw, bh] = t.frame;
    // text-local pixels (a line's middle at its y, its advance centred on x = 0) → canvas → clip → target
    let centre = Vec2::new(w / 2.0 + t.center.x * w, h / 2.0 + t.center.y * h);
    let local = Affine::translate(centre.x, centre.y).then_apply(&Affine::scale(t.scale, t.scale)).then_apply(&Affine::rotate_deg(t.rotation));
    let to = clip_to.then_apply(&Affine::translate(bx, by)).then_apply(&Affine::scale(bw / w, bh / h)).then_apply(&local);
    let x: raster::Xform = [to.a as f32, to.b as f32, to.c as f32, to.d as f32, to.e as f32, to.f as f32];
    let style = TextStyle {
        family: t.family.clone(),
        style: match (t.bold, t.italic) {
            (false, false) => "Regular",
            (true, false) => "Bold",
            (false, true) => "Italic",
            (true, true) => "Bold Italic",
        }
        .into(),
        size: px as f32,
        tracking: (t.letter_spacing / px * 1000.0) as f32,
        kerning: true,
        ligatures: true,
        ..TextStyle::default()
    };
    let para = ParagraphStyle { align: Align::Center, ..ParagraphStyle::default() };
    let (over, under) = filmcraft_text::fonts::face(filmcraft_text::resolve(&t.family, &style.style).face).em_box(px as f32);
    let middle = ((over - under) / 2.0) as f64;
    let lines: Vec<&str> = t.content.split('\n').collect();
    let pitch = px * t.line_height;
    let first = -((lines.len() - 1) as f64) * pitch / 2.0;
    let thickness = (px * DECORATION_THICKNESS).max(1.0);
    let positive = to.determinant() >= 0.0;
    let mut out: Vec<Vec<[f32; 2]>> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let mid_y = first + i as f64 * pitch;
        let l = filmcraft_text::layout(line, &style, &para);
        // the line's ink about its middle (y down, `None` without ink), which places its decoration
        let mut ink: Option<(f64, f64)> = None;
        for g in &l.glyphs {
            let gm: raster::Xform = raster::compose(&x, &[1.0, 0.0, 0.0, 1.0, g.x, g.y + (mid_y + middle) as f32]);
            let path = filmcraft_text::render::glyph_path(g, &gm);
            if t.decoration != TextDecoration::None
                && let Some((_, y0, _, y1)) = filmcraft_text::render::glyph_path(g, &[1.0, 0.0, 0.0, 1.0, g.x, g.y + middle as f32]).bounds()
            {
                let (a, b) = ink.unwrap_or((f64::MAX, f64::MIN));
                ink = Some((a.min(y0 as f64), b.max(y1 as f64)));
            }
            let polys: Vec<Vec<[f32; 2]>> = path.contours.iter().map(|c| c.iter().map(|&(a, b)| [a, b]).collect()).collect();
            // the glyph's outer contour (its largest) wound as every other shape's, its counters against it
            let flip = polys.iter().map(|p| area(p)).max_by(|a, b| a.abs().total_cmp(&b.abs())).is_some_and(|a| (a >= 0.0) != positive);
            out.extend(polys.into_iter().map(|mut p| {
                if flip {
                    p.reverse();
                }
                p
            }));
        }
        if t.decoration == TextDecoration::None {
            continue;
        }
        let (x0, x1) = l.lines.first().map_or((0.0, 0.0), |ln| ln.carets.iter().fold((f32::MAX, f32::MIN), |(a, b), c| (a.min(c.1), b.max(c.1))));
        if x1 <= x0 {
            continue;
        }
        let (ascent, descent) = ink.map_or((0.0, 0.0), |(top, bottom)| (-top, bottom));
        let y0 = match t.decoration {
            TextDecoration::Underline => mid_y + descent + thickness,
            _ => mid_y - (ascent - descent) * STRIKE_RISE,
        };
        let rect: Vec<[f32; 2]> = [(x0 as f64, y0), (x1 as f64, y0), (x1 as f64, y0 + thickness), (x0 as f64, y0 + thickness)]
            .into_iter()
            .map(|(a, b)| to.apply(Vec2::new(a, b)))
            .map(|q| [q.x as f32, q.y as f32])
            .collect();
        out.push(wound(rect, positive));
    }
    out
}

/// Whether an item's Opacity masks are drawn after its Motion, on the sequence frame's grid: when one of them is measured in the
/// canvas's pixels, so its feather is as sharp as the frame (Cut draws its masks on its canvas; OneOS doctrine files/989, tool choice).
/// A clip holding only clip-pixel masks draws them on its own pixels before Motion.
pub fn opacity_masks_after_motion(item: &filmcraft_project::TrackItem) -> bool {
    item.effect("opacity").is_some_and(|e| e.enabled && e.masks.iter().any(|m| m.mode != MaskMode::None && m.units == MaskUnits::Canvas))
}

/// `metric` applied to the offset (x, y).
#[inline]
fn measured(m: &[f32; 4], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y, m[1] * x + m[3] * y)
}

/// The least stretch of `metric` (its smaller singular value): a band of `b` measured pixels reaches at most `b / least` working
/// pixels from the edge.
fn least_stretch(m: &[f32; 4]) -> f32 {
    let t = m.iter().map(|v| v * v).sum::<f32>();
    let det = (m[0] * m[3] - m[1] * m[2]).abs();
    let most = ((t + (t * t - 4.0 * det * det).max(0.0).sqrt()) / 2.0).sqrt();
    if most > 0.0 { det / most } else { 0.0 }
}

/// The falloff of a signed (expanded) distance for a feather width.
#[inline]
pub fn falloff(s: f32, feather: f32) -> f32 {
    let w = feather.max(1.0);
    let u = (s / w + 0.5).clamp(0.0, 1.0);
    let smooth = u * u * (3.0 - 2.0 * u);
    u + (smooth - u) * feather.min(1.0)
}

/// Distance band beyond which coverage is saturated (0 or 1).
fn band(m: &FlatMask) -> f32 {
    m.expansion.abs() + m.feather.max(1.0) * 0.5 + 1.0
}

/// The squared distance from (px, py) to the segment a–b, measured through `metric`.
#[inline]
fn seg_dist2(px: f32, py: f32, a: [f32; 2], b: [f32; 2], metric: &[f32; 4]) -> f32 {
    let (dx, dy) = measured(metric, b[0] - a[0], b[1] - a[1]);
    let (qx, qy) = measured(metric, px - a[0], py - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { ((qx * dx + qy * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (ex, ey) = (qx - dx * t, qy - dy * t);
    ex * ex + ey * ey
}

/// Every edge of every contour of `m` (each contour closed).
fn edges(m: &FlatMask) -> impl Iterator<Item = ([f32; 2], [f32; 2])> + '_ {
    m.contours.iter().flat_map(|c| (0..c.len()).map(move |i| (c[i], c[(i + 1) % c.len()])))
}

/// Coverage of one mask for row `y` into `out` (length = width). Reference per-row algorithm:
/// winding from the row's sorted edge crossings, distance only against edges within the band.
fn mask_row(m: &FlatMask, y: usize, out: &mut [f32]) {
    let py = y as f32 + 0.5;
    let b = band(m);
    // the band in working pixels (where edges are culled); a degenerate map (a clip scaled to nothing) culls none
    let least = least_stretch(&m.metric);
    let reach = if least > 0.0 { b / least } else { f32::INFINITY };
    let value = |sd: f32| {
        let c = falloff(sd + m.expansion, m.feather);
        m.opacity * if m.inverted { 1.0 - c } else { c }
    };
    if m.contours.is_empty() {
        out.fill(value(-b));
        return;
    }
    // crossings of the scanline with the polygon edges (x, direction)
    let mut cross: Vec<(f32, i32)> = Vec::new();
    // edges within the band of this row: (a, b, min x, max x)
    let mut near: Vec<([f32; 2], [f32; 2], f32, f32)> = Vec::new();
    for (a, c) in edges(m) {
        if (a[1] <= py) != (c[1] <= py) {
            let t = (py - a[1]) / (c[1] - a[1]);
            cross.push((a[0] + (c[0] - a[0]) * t, if c[1] > a[1] { 1 } else { -1 }));
        }
        if py >= a[1].min(c[1]) - reach && py <= a[1].max(c[1]) + reach {
            near.push((a, c, a[0].min(c[0]) - reach, a[0].max(c[0]) + reach));
        }
    }
    cross.sort_by(|p, q| p.0.total_cmp(&q.0));
    let mut ci = 0;
    let mut wind = 0;
    let b2 = b * b;
    for (x, o) in out.iter_mut().enumerate() {
        let px = x as f32 + 0.5;
        while ci < cross.len() && cross[ci].0 < px {
            wind += cross[ci].1;
            ci += 1;
        }
        let inside = wind != 0;
        let mut d2 = b2;
        for (a, c, x0, x1) in &near {
            if px >= *x0 && px <= *x1 {
                d2 = d2.min(seg_dist2(px, py, *a, *c, &m.metric));
            }
        }
        let d = d2.sqrt();
        *o = value(if inside { d } else { -d });
    }
}

/// Combined coverage of `masks` over a `w`×`h` image, or `None` when there is no active mask
/// (the effect applies everywhere).
pub fn coverage(masks: &[FlatMask], w: usize, h: usize) -> Option<Vec<f32>> {
    let first = masks.iter().find(|m| m.mode != MaskMode::None)?;
    let start = first.mode.start();
    let mut acc = vec![start; w * h];
    if w == 0 {
        return Some(acc);
    }
    acc.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let mut tmp = vec![0.0f32; w];
        for m in masks.iter().filter(|m| m.mode != MaskMode::None) {
            mask_row(m, y, &mut tmp);
            for (a, v) in row.iter_mut().zip(&tmp) {
                *a = m.mode.combine(*a, *v).clamp(0.0, 1.0);
            }
        }
    });
    Some(acc)
}

/// Coverage of an effect's masks on a working image (`px_scale` working px per clip px, `to_canvas` working px → sequence px).
pub fn effect_coverage(masks: &[Mask], t: Tick, px_scale: f32, to_canvas: &Affine, w: usize, h: usize) -> Option<Vec<f32>> {
    if masks.is_empty() {
        return None;
    }
    coverage(&prepare(masks, t, px_scale, to_canvas), w, h)
}

/// `img = lerp(original, img, cov)` on premultiplied pixels.
pub fn mix(img: &mut Image, original: &Image, cov: &[f32]) {
    img.px.par_chunks_mut(4).zip(original.px.par_chunks(4)).zip(cov.par_iter()).for_each(|((p, o), &c)| {
        for k in 0..4 {
            p[k] = o[k] + (p[k] - o[k]) * c;
        }
    });
}

/// Scale premultiplied pixels by coverage (opacity masks).
pub fn scale_by(img: &mut Image, cov: &[f32]) {
    img.px.par_chunks_mut(4).zip(cov.par_iter()).for_each(|(p, &c)| {
        for v in p {
            *v *= c;
        }
    });
}

/// Apply an effect limited to its masks (Premiere: a masked effect only applies inside the mask).
pub fn apply_effect(img: &mut Image, e: &EffectInstance, cx: &crate::effects::FxCtx) {
    if !e.enabled || e.masks.is_empty() {
        crate::effects::apply(img, e, cx);
        return;
    }
    // The working image's map to the sequence frame: through the clip's Motion, or, where the effect has no clip around it (an
    // adjustment layer, whose working image is the sequence frame at `px_scale`), the scale alone.
    let to_canvas = match cx.env {
        Some(env) => env.layer_to_sequence(),
        None => Affine::scale(1.0 / cx.px_scale as f64, 1.0 / cx.px_scale as f64),
    };
    let Some(cov) = effect_coverage(&e.masks, cx.t, cx.px_scale, &to_canvas, img.w, img.h) else {
        crate::effects::apply(img, e, cx);
        return;
    };
    let original = img.clone();
    crate::effects::apply(img, e, cx);
    mix(img, &original, &cov);
}

/// Apply the masks of an item's Opacity effect to its layer (before Motion), if any; `to_canvas` maps the layer's pixels to the
/// sequence frame's.
pub fn apply_opacity_masks(img: &mut Image, item: &filmcraft_project::TrackItem, t: Tick, px_scale: f32, to_canvas: &Affine) {
    let Some(op) = item.effect("opacity").filter(|e| e.enabled && !e.masks.is_empty()) else { return };
    if let Some(cov) = effect_coverage(&op.masks, t, px_scale, to_canvas, img.w, img.h) {
        scale_by(img, &cov);
    }
}

/// Brute-force coverage of one pixel (every edge; the definition the row algorithm and the WGSL
/// shader implement). For tests and documentation.
pub fn coverage_at(masks: &[FlatMask], x: f32, y: f32) -> Option<f32> {
    let first = masks.iter().find(|m| m.mode != MaskMode::None)?;
    let mut acc = first.mode.start();
    for m in masks.iter().filter(|m| m.mode != MaskMode::None) {
        let empty = m.contours.is_empty();
        let mut wind = 0;
        let mut d2 = f32::INFINITY;
        for (a, c) in edges(m) {
            if (a[1] <= y) != (c[1] <= y) {
                let t = (y - a[1]) / (c[1] - a[1]);
                if a[0] + (c[0] - a[0]) * t < x {
                    wind += if c[1] > a[1] { 1 } else { -1 };
                }
            }
            d2 = d2.min(seg_dist2(x, y, a, c, &m.metric));
        }
        let d = if empty { band(m) } else { d2.sqrt().min(band(m)) };
        let sd = if wind != 0 && !empty { d } else { -d };
        let c = falloff(sd + m.expansion, m.feather);
        let v = m.opacity * if m.inverted { 1.0 - c } else { c };
        acc = m.mode.combine(acc, v).clamp(0.0, 1.0);
    }
    Some(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_geom::{Affine, Vec2};
    use filmcraft_project::{MaskPath, MaskUnits};

    fn mask(path: MaskPath, feather: f64) -> Mask {
        let mut m = Mask::new("m", path);
        m.feather.value = filmcraft_project::ParamValue::Float(feather);
        m
    }

    #[test]
    fn hard_rect_is_exact_box_coverage() {
        // rect edges on pixel boundaries → exactly 1 inside, 0 outside; half-pixel edge → 0.5
        let m = mask(MaskPath::rect(2.0, 2.0, 6.5, 6.0), 0.0);
        let c = effect_coverage(&[m], Tick(0), 1.0, &Affine::IDENTITY, 10, 10).unwrap();
        assert_eq!(c[3 * 10 + 3], 1.0);
        assert_eq!(c[3 * 10 + 1], 0.0);
        assert!((c[3 * 10 + 6] - 0.5).abs() < 1e-6, "{}", c[3 * 10 + 6]);
        assert_eq!(c[8 * 10 + 3], 0.0);
    }

    #[test]
    fn ellipse_area_matches_analytic() {
        let (rx, ry) = (37.3, 21.7);
        let m = mask(MaskPath::ellipse(Vec2::new(50.2, 40.1), Vec2::new(rx, ry)), 0.0);
        let c = effect_coverage(&[m], Tick(0), 1.0, &Affine::IDENTITY, 100, 80).unwrap();
        let area: f64 = c.iter().map(|v| *v as f64).sum();
        let exact = std::f64::consts::PI * rx * ry;
        assert!((area - exact).abs() / exact < 0.002, "{area} vs {exact}");
    }

    #[test]
    fn feather_is_a_symmetric_distance_falloff() {
        let m = mask(MaskPath::rect(20.0, -100.0, 200.0, 200.0), 10.0);
        let c = effect_coverage(&[m], Tick(0), 1.0, &Affine::IDENTITY, 60, 4).unwrap();
        // edge at x = 20: 0.5 there, saturated 5 px away, symmetric
        let at = |x: usize| c[x];
        assert!((at(19) + at(20) - 1.0).abs() < 1e-5, "{} {}", at(19), at(20));
        assert!(at(13) < 1e-6 && at(26) > 1.0 - 1e-6, "{} {}", at(13), at(26));
        for x in 14..26 {
            assert!(at(x + 1) >= at(x), "monotonic");
        }
    }

    #[test]
    fn expansion_inverted_opacity_modes() {
        let mut m = mask(MaskPath::rect(10.0, 10.0, 20.0, 20.0), 0.0);
        m.expansion.value = filmcraft_project::ParamValue::Float(5.0);
        let c = effect_coverage(std::slice::from_ref(&m), Tick(0), 1.0, &Affine::IDENTITY, 30, 30).unwrap();
        assert_eq!(c[15 * 30 + 6], 1.0, "expanded by 5");
        assert_eq!(c[15 * 30 + 4], 0.0);
        m.inverted = true;
        m.opacity.value = filmcraft_project::ParamValue::Float(50.0);
        let c = effect_coverage(&[m.clone()], Tick(0), 1.0, &Affine::IDENTITY, 30, 30).unwrap();
        assert_eq!(c[15 * 30 + 15], 0.0);
        assert_eq!(c[2], 0.5);
        // subtract a hole from a big rect
        let mut big = mask(MaskPath::rect(0.0, 0.0, 30.0, 30.0), 0.0);
        big.mode = MaskMode::Add;
        let mut hole = mask(MaskPath::rect(10.0, 10.0, 20.0, 20.0), 0.0);
        hole.mode = MaskMode::Subtract;
        let c = effect_coverage(&[big, hole], Tick(0), 1.0, &Affine::IDENTITY, 30, 30).unwrap();
        assert_eq!(c[15 * 30 + 15], 0.0);
        assert_eq!(c[5 * 30 + 5], 1.0);
    }

    #[test]
    fn row_algorithm_matches_brute_force() {
        let mut a = mask(MaskPath::ellipse(Vec2::new(30.0, 25.0), Vec2::new(20.0, 12.0)), 6.0);
        a.expansion.value = filmcraft_project::ParamValue::Float(-2.0);
        let mut b = mask(MaskPath::polygon(&[Vec2::new(5.0, 5.0), Vec2::new(50.0, 10.0), Vec2::new(20.0, 45.0)]), 0.0);
        b.mode = MaskMode::Difference;
        b.inverted = true;
        let flat = prepare(&[a, b], Tick(0), 0.8, &Affine::IDENTITY);
        let (w, h) = (48, 40);
        let c = coverage(&flat, w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let r = coverage_at(&flat, x as f32 + 0.5, y as f32 + 0.5).unwrap();
                assert!((r - c[y * w + x]).abs() < 1e-5, "({x},{y}) {r} vs {}", c[y * w + x]);
            }
        }
    }

    fn canvas(path: MaskPath, feather: f64) -> Mask {
        let mut m = mask(path, feather);
        m.units = MaskUnits::Canvas;
        m
    }

    /// files/989 (Mona 2026-11-09: "Build canvas feather in Film"): a mask measured in the canvas's pixels is feathered across its edge
    /// in the sequence frame's pixels, through the clip's own map to them, so on an uneven scale (two working pixels across, half a
    /// working pixel down, per canvas pixel) its 10-pixel feather is 5 working pixels across and 20 down: exactly the clip-pixel masks
    /// of those widths.
    #[test]
    fn a_canvas_feather_is_measured_in_the_canvas_s_pixels_through_an_uneven_scale() {
        let to_canvas = Affine::scale(2.0, 0.5);
        let rect = MaskPath::rect(20.0, 30.0, 400.0, 400.0);
        let (w, h) = (60, 80);
        let c = effect_coverage(&[canvas(rect.clone(), 10.0)], Tick(0), 1.0, &to_canvas, w, h).unwrap();
        let across = effect_coverage(&[mask(rect.clone(), 5.0)], Tick(0), 1.0, &Affine::IDENTITY, w, h).unwrap();
        let down = effect_coverage(&[mask(rect.clone(), 20.0)], Tick(0), 1.0, &Affine::IDENTITY, w, h).unwrap();
        for x in 10..30 {
            let i = 70 * w + x;
            assert!((c[i] - across[i]).abs() < 1e-5, "across at {x}: {} where 5 clip pixels give {}", c[i], across[i]);
        }
        for y in 10..50 {
            let i = y * w + 55;
            assert!((c[i] - down[i]).abs() < 1e-5, "down at {y}: {} where 20 clip pixels give {}", c[i], down[i]);
        }
        // A clip-pixel mask ignores the map: Film's own masks draw as they always did.
        let clip = effect_coverage(&[mask(rect, 5.0)], Tick(0), 1.0, &to_canvas, w, h).unwrap();
        assert_eq!(clip, across);
    }

    /// Rotation keeps lengths, so through a rotated, evenly scaled map a canvas feather is one width in working pixels everywhere,
    /// expansion included (9 canvas pixels at 1.5 canvas pixels per working pixel are 6).
    #[test]
    fn a_canvas_feather_through_a_rotation_is_one_width_all_round() {
        let to_canvas = Affine::rotate_deg(30.0).then_apply(&Affine::scale(1.5, 1.5));
        let shape = MaskPath::ellipse(Vec2::new(40.0, 32.0), Vec2::new(22.0, 14.0));
        let mut a = canvas(shape.clone(), 9.0);
        a.expansion.value = filmcraft_project::ParamValue::Float(-3.0);
        let mut b = mask(shape, 6.0);
        b.expansion.value = filmcraft_project::ParamValue::Float(-2.0);
        let c = effect_coverage(&[a], Tick(0), 1.0, &to_canvas, 80, 64).unwrap();
        let d = effect_coverage(&[b], Tick(0), 1.0, &Affine::IDENTITY, 80, 64).unwrap();
        let worst = c.iter().zip(&d).map(|(p, q)| (p - q).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-4, "{worst}");
    }

    /// The row algorithm and the brute-force definition agree on canvas masks through a skewed, uneven map too (the GPU shader is the
    /// brute-force definition).
    #[test]
    fn row_algorithm_matches_brute_force_in_the_canvas_s_pixels() {
        let to_canvas = Affine { a: 1.7, b: 0.4, c: -0.3, d: 0.6, e: 11.0, f: -4.0 };
        let mut a = canvas(MaskPath::ellipse(Vec2::new(30.0, 25.0), Vec2::new(20.0, 12.0)), 7.0);
        a.expansion.value = filmcraft_project::ParamValue::Float(2.5);
        let mut b = canvas(MaskPath::polygon(&[Vec2::new(5.0, 5.0), Vec2::new(50.0, 10.0), Vec2::new(20.0, 45.0)]), 0.0);
        b.mode = MaskMode::Subtract;
        let flat = prepare(&[a, b, mask(MaskPath::rect(8.0, 8.0, 30.0, 30.0), 3.0)], Tick(0), 0.8, &to_canvas);
        let (w, h) = (48, 40);
        let c = coverage(&flat, w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let r = coverage_at(&flat, x as f32 + 0.5, y as f32 + 0.5).unwrap();
                assert!((r - c[y * w + x]).abs() < 1e-5, "({x},{y}) {r} vs {}", c[y * w + x]);
            }
        }
    }

    // --- text masks (OneOS doctrine files/989, tool choice: Film draws Cut's text mask as Cut does) ---

    fn text_mask(content: &str, frame: [f64; 4], size: f64) -> Mask {
        let mut t = filmcraft_project::MaskText::new(content, frame);
        t.family = "Inter".into();
        t.size = size;
        let mut m = Mask::text("Text", t);
        m.feather.value = filmcraft_project::ParamValue::Float(0.0);
        m
    }

    fn edit(mut m: Mask, f: impl FnOnce(&mut filmcraft_project::MaskText)) -> Mask {
        f(m.text.as_mut().unwrap());
        m
    }

    /// The box `[x0, y0, x1, y1]` of the pixels a coverage holds above one half.
    fn ink(c: &[f32], w: usize) -> [f64; 4] {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for (i, v) in c.iter().enumerate() {
            if *v > 0.5 {
                let (x, y) = ((i % w) as f64, (i / w) as f64);
                b = [b[0].min(x), b[1].min(y), b[2].max(x + 1.0), b[3].max(y + 1.0)];
            }
        }
        b
    }

    /// The middle of Inter's em box above its baseline at `px` (where a browser's `textBaseline = "middle"` sets a line).
    fn middle(px: f64) -> f64 {
        let (over, under) = filmcraft_text::fonts::face(filmcraft_text::resolve("Inter", "Regular").face).em_box(px as f32);
        ((over - under) / 2.0) as f64
    }

    fn cover(m: &Mask, to_canvas: &Affine, w: usize, h: usize) -> Vec<f32> {
        coverage(&prepare(std::slice::from_ref(m), Tick(0), 1.0, to_canvas), w, h).unwrap()
    }

    #[test]
    fn a_text_mask_is_its_glyphs_with_their_counters_set_on_the_middle_of_the_em_box() {
        // size 90 on a 300-pixel-high box: a 300-pixel font, its line's middle on the box's centre
        let (w, h) = (400usize, 300usize);
        let m = text_mask("O", [0.0, 0.0, w as f64, h as f64], 90.0);
        let c = cover(&m, &Affine::IDENTITY, w, h);
        let [x0, y0, x1, y1] = ink(&c, w);
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        assert!((cx - 200.0).abs() < 3.0, "the O is centred on the box: {x0}..{x1}");
        let baseline = 150.0 + middle(300.0);
        assert!((y1 - baseline).abs() < 0.02 * 300.0, "the O sits on the baseline the em box's middle puts at {baseline}: {y0}..{y1}");
        let at = |x: f64, y: f64| c[y as usize * w + x as usize];
        assert_eq!(at(cx, cy), 0.0, "the O's counter is a hole");
        assert_eq!(at(x0 + 4.0, cy), 1.0, "its ring is filled");
        assert_eq!(at(5.0, 5.0), 0.0);
    }

    #[test]
    fn a_text_mask_s_lines_are_line_height_apart_about_its_centre_moved_scaled_and_turned_as_cut_s() {
        let (w, h) = (400usize, 300usize);
        // a 100-pixel font, lines 150 pixels apart: their middles at 75 and 225
        let m = edit(text_mask("I\nI", [0.0, 0.0, w as f64, h as f64], 30.0), |t| t.line_height = 1.5);
        let c = cover(&m, &Affine::IDENTITY, w, h);
        let (top, bottom) = c.split_at(150 * w);
        let (a, b) = (ink(top, w), ink(bottom, w));
        let base = middle(100.0);
        assert!((a[3] - (75.0 + base)).abs() < 1.0 && (b[3] + 150.0 - (225.0 + base)).abs() < 1.0, "{a:?} {b:?}");
        // moved a quarter of the box right, doubled and turned a quarter turn: the stem lies along x, centred at (300, 150)
        let one = edit(text_mask("I", [0.0, 0.0, w as f64, h as f64], 30.0), |t| {
            t.center = Vec2::new(0.25, 0.0);
            t.scale = 2.0;
            t.rotation = 90.0;
        });
        let plain = ink(&cover(&edit(one.clone(), |t| (t.scale, t.rotation) = (1.0, 0.0)), &Affine::IDENTITY, w, h), w);
        let turned = ink(&cover(&one, &Affine::IDENTITY, w, h), w);
        let (pw, ph, tw, th) = (plain[2] - plain[0], plain[3] - plain[1], turned[2] - turned[0], turned[3] - turned[1]);
        // (a pixel of edge either side of the doubled stem)
        assert!((tw - 2.0 * ph).abs() <= 3.0 && (th - 2.0 * pw).abs() <= 3.0, "turned and doubled: {plain:?} {turned:?}");
        // turned about the text's centre: its middle line's point on the stem stays at x = 300 after a quarter turn about it
        let stem_mid_y = (plain[1] + plain[3]) / 2.0;
        assert!(((turned[0] + turned[2]) / 2.0 - (300.0 - 2.0 * (stem_mid_y - 150.0))).abs() < 2.0, "{turned:?}");
    }

    #[test]
    fn a_canvas_text_mask_is_drawn_on_the_box_s_size_on_screen_so_an_uneven_scale_does_not_stretch_its_glyphs() {
        // Cut draws a canvas-pixel mask on a canvas the element's drawn size, its font from that canvas's height: on a clip shown 200 %
        // wide and 100 % high, the text is as tall in clip pixels and half as wide as the clip-pixel mask's.
        let (w, h) = (400usize, 300usize);
        let clip = text_mask("H", [0.0, 0.0, w as f64, h as f64], 30.0);
        let mut canvas = clip.clone();
        canvas.units = MaskUnits::Canvas;
        let a = ink(&cover(&clip, &Affine::scale(2.0, 1.0), w, h), w);
        let b = ink(&cover(&canvas, &Affine::scale(2.0, 1.0), w, h), w);
        assert!(((b[3] - b[1]) - (a[3] - a[1])).abs() <= 1.0, "as tall: {a:?} {b:?}");
        assert!(((b[2] - b[0]) / (a[2] - a[0]) - 0.5).abs() < 0.04, "half as wide: {a:?} {b:?}");
        // through an even scale of 2 the canvas is twice the box, so the font is twice the clip-pixel mask's, laid back on the box
        let even = ink(&cover(&canvas, &Affine::scale(2.0, 2.0), w, h), w);
        assert!(((even[3] - even[1]) - (a[3] - a[1])).abs() <= 1.0, "{a:?} {even:?}");
    }

    #[test]
    fn a_strike_through_fills_over_its_glyph_and_an_underline_sits_its_thickness_below_the_ink() {
        let (w, h) = (300usize, 180usize);
        // a 100-pixel font (size 50 of 180): Cut's decoration is 7 pixels thick
        let o = text_mask("o", [0.0, 0.0, w as f64, h as f64], 50.0);
        let plain = cover(&o, &Affine::IDENTITY, w, h);
        let [x0, y0, x1, y1] = ink(&plain, w);
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let at = |c: &[f32], x: f64, y: f64| c[y as usize * w + x as usize];
        // the strike: (ascent − descent) · 0.35 above the line's middle (90), its ink measured from that middle
        let (ascent, descent) = (90.0 - y0, y1 - 90.0);
        let strike = 90.0 - (ascent - descent) * 0.35 + 3.5;
        let struck = cover(&edit(o.clone(), |t| t.decoration = filmcraft_project::TextDecoration::LineThrough), &Affine::IDENTITY, w, h);
        assert_eq!(at(&plain, cx, strike), 0.0, "the counter, unstruck");
        assert_eq!(at(&struck, cx, strike), 1.0, "struck across the counter");
        assert_eq!(at(&struck, x0 + 3.0, strike), 1.0, "where the strike crosses the ring both fill as one");
        assert_eq!(at(&struck, x0 - 3.0, cy + 15.0), 0.0);
        let under = cover(&edit(o, |t| t.decoration = filmcraft_project::TextDecoration::Underline), &Affine::IDENTITY, w, h);
        let line = 90.0 + descent + 7.0;
        assert_eq!(at(&under, cx, line + 3.5), 1.0, "the underline, 7 pixels below the ink");
        assert_eq!(at(&under, cx, line - 3.5), 0.0, "a gap of its thickness above it");
    }

    #[test]
    fn a_text_mask_without_text_is_not_drawn() {
        let mut m = text_mask("  \n ", [0.0, 0.0, 100.0, 100.0], 30.0);
        m.mode = MaskMode::Intersect;
        assert!(prepare(&[m], Tick(0), 1.0, &Affine::IDENTITY).is_empty(), "inactive, as Cut's (an intersect would otherwise empty the clip)");
    }

    #[test]
    fn row_algorithm_matches_brute_force_on_a_text_mask() {
        let mut a = edit(text_mask("Ag\nB8", [0.0, 0.0, 64.0, 48.0], 30.0), |t| t.rotation = 17.0);
        a.feather.value = filmcraft_project::ParamValue::Float(2.5);
        a.expansion.value = filmcraft_project::ParamValue::Float(0.75);
        a.units = MaskUnits::Canvas;
        let mut b = mask(MaskPath::rect(10.0, 10.0, 40.0, 30.0), 1.0);
        b.mode = MaskMode::Difference;
        let flat = prepare(&[a, b], Tick(0), 1.0, &Affine::scale(1.3, 0.9));
        assert!(flat[0].contours.len() > 6, "{} contours", flat[0].contours.len());
        let (w, h) = (64, 48);
        let c = coverage(&flat, w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let r = coverage_at(&flat, x as f32 + 0.5, y as f32 + 0.5).unwrap();
                assert!((r - c[y * w + x]).abs() < 1e-5, "({x},{y}) {r} vs {}", c[y * w + x]);
            }
        }
    }
}
