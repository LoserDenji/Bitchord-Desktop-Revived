//! The now-playing backdrop ported from ForkChord's full-screen player (`ArtworkBackdrop`,
//! itself a port of the Kotlin app's `ArtworkMeshBackdrop.kt`). Two layers over a dark
//! base, both from one 120 px decode of the cover:
//!
//!  - the mesh: the artwork averaged into a 6×6 grid of cell means (proportions survive:
//!    nine-tenths black stays nine-tenths black), each cell's saturation nudged up and its
//!    lightness floored off pure black, the rows below the seam cyclic-shifted sideways
//!    (and mirrored half the time, seeded per artwork), the grid resampled to 32×32 with a
//!    curve that flattens at every control point, and the texture stretched over the view
//!    a little oversize — no creases when magnified.
//!  - the full-cover blur over it: the same decode box-blurred three passes at radius
//!    min(w,h)/12, drawn cropped across the whole surface under a darker legibility scrim —
//!    the layer the lyrics and the queue live on.
//!
//! Both crossfade on a track change and never flicker to empty while a decode is in
//! flight: what's on screen is held until the new one is ready. Never animated: a still
//! wallpaper, like the reference.

use std::collections::HashMap;

use egui::{Color32, Rect, Ui, pos2, vec2};

use crate::model::Thumb;

/// Cells across the mesh. Coarse deliberately, so the cover's own proportions survive.
const MESH_GRID: usize = 6;
/// The artwork decodes at this square size before anything reads it.
const MESH_PX: u32 = 120;
/// The grid is interpolated to this texture before egui stretches it (bilinear).
const MESH_TEX: usize = 32;
/// How long the mesh crossfade runs.
const MESH_FADE: f64 = 0.9;
/// Saturation nudged back up, lightness floored off pure black.
const MESH_VIBRANCE: f32 = 1.12;
const MESH_FLOOR: f32 = 0.045;
/// The full-cover blur's passes, and how long it fades in over the mesh.
const FULL_BLUR_PASSES: usize = 3;
const FULL_BLUR_FADE: f64 = 0.32;
/// How far the mesh reaches past the view (the reference's `-inset-[6%]`).
const MESH_OVERSCAN: f32 = 0.06;
/// The surface before any artwork has been read. Never a colour anyone chose.
const FALLBACK: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);

/// One decoded cover: its mesh texture and its full-cover blur.
#[derive(Clone)]
struct Entry {
    mesh: egui::TextureHandle,
    blur: egui::TextureHandle,
}

#[derive(Clone)]
enum Slot {
    Ready(Entry),
    /// Its bytes are still downloading: try again next frame.
    Pending,
    /// It never decoded: stop asking.
    Failed,
}

/// What the backdrop last showed, so a track change crossfades instead of cutting.
#[derive(Clone)]
struct Shown {
    url: String,
    mesh: egui::TextureHandle,
    /// The full-cover blur on screen, if it isn't on its way out. A track whose blur
    /// isn't cached fades the old one out first (the mesh shows), the way the reference
    /// animates to 0 while the image is null; a cached one swaps without a fade.
    blur: Option<egui::TextureHandle>,
    /// Where the blur layer is heading, and when it set off (for its 320 ms move).
    blur_up: bool,
    blur_since: f64,
    /// The mesh that was up before this one, and when the crossfade started.
    from: Option<(egui::TextureHandle, f64)>,
}

/// Java String.hashCode, over UTF-16 units — the seed the reference's arrangement is
/// driven by, so the same artwork lands in the same arrangement every time.
fn hash_url(url: &str) -> i32 {
    let mut hash: i32 = 0;
    for unit in url.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
    }
    hash
}

/// java.util.Random, whose LCG the reference's random runs on — ported so the same
/// artwork keeps the same arrangement.
struct JavaRandom {
    seed: i64,
}

impl JavaRandom {
    const MULTIPLIER: i64 = 0x5DEECE66D;
    const MASK: i64 = (1 << 48) - 1;

    fn new(seed: i32) -> Self {
        Self { seed: (i64::from(seed) ^ Self::MULTIPLIER) & Self::MASK }
    }

    fn next(&mut self, bits: u32) -> i64 {
        self.seed = self.seed.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB) & Self::MASK;
        self.seed >> (48 - bits)
    }

    fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }

    fn next_int(&mut self, bound: usize) -> usize {
        assert!(bound > 0);
        // java.util.Random.nextInt: rejection-sample until the modulo is fair.
        loop {
            let bits = self.next(31) as u64;
            let value = bits % bound as u64;
            // The overflow check that keeps the modulo fair on any bound.
            if bits - value + (bound as u64 - 1) <= 0x7fff_ffff {
                return value as usize;
            }
        }
    }
}

type Cell = [u8; 3];

/// Averages the decoded pixels (RGB, width×height) into the grid, flipped top to bottom
/// as it is read — row 0 of the result is the artwork's own bottom edge, the order the
/// backdrop draws down the screen.
fn mesh_cells(px: &[u8], width: usize, height: usize) -> Vec<Cell> {
    let cols = MESH_GRID.min(width);
    let rows = MESH_GRID.min(height);
    let row_step = 1.max(height / 128);
    let col_step = 1.max(width / 128);

    let mut red = vec![0.0f64; rows * cols];
    let mut green = vec![0.0f64; rows * cols];
    let mut blue = vec![0.0f64; rows * cols];
    let mut count = vec![0usize; rows * cols];

    for y in (0..height).step_by(row_step) {
        let row_base = ((height - 1 - y) * rows / height) * cols;
        for x in (0..width).step_by(col_step) {
            let cell = row_base + x * cols / width;
            let at = (y * width + x) * 3;
            red[cell] += f64::from(px[at]);
            green[cell] += f64::from(px[at + 1]);
            blue[cell] += f64::from(px[at + 2]);
            count[cell] += 1;
        }
    }

    (0..rows * cols)
        .map(|cell| {
            let n = count[cell].max(1) as f64;
            [(red[cell] / n).round() as u8, (green[cell] / n).round() as u8, (blue[cell] / n).round() as u8]
        })
        .collect()
}

/// The one liberty taken with the artwork's colours: saturation up a touch, lightness
/// never quite black.
fn lift_cell(c: Cell) -> Cell {
    let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
    let s = (s * MESH_VIBRANCE).min(1.0);
    let l = l.max(MESH_FLOOR);
    hsl_to_rgb(h, s, l)
}

/// The reference's RGBToHSL, ported as written, so the numbers do not move.
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (rf, gf, bf) = (f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;
    let l = (max + min) / 2.0;

    let (mut h, s) = if max == min {
        (0.0, 0.0)
    } else {
        let h = if max == rf {
            ((gf - bf) / delta) % 6.0
        } else if max == gf {
            (bf - rf) / delta + 2.0
        } else {
            (rf - gf) / delta + 4.0
        };
        let s = delta / (1.0 - (2.0 * l - 1.0).abs());
        (h, s)
    };

    h = (h * 60.0) % 360.0;
    if h < 0.0 {
        h += 360.0;
    }
    (h.clamp(0.0, 360.0), s.clamp(0.0, 1.0), l.clamp(0.0, 1.0))
}

/// The reference's HSLToColor, same port.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Cell {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let m = l - 0.5 * c;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let segment = (h / 60.0).trunc().floor();

    let (r, g, b) = if segment == 0.0 {
        (c + m, x + m, m)
    } else if segment == 1.0 {
        (x + m, c + m, m)
    } else if segment == 2.0 {
        (m, c + m, x + m)
    } else if segment == 3.0 {
        (m, x + m, c + m)
    } else if segment == 4.0 {
        (x + m, m, c + m)
    } else {
        (c + m, m, x + m)
    };

    let round = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    [round(r * 255.0), round(g * 255.0), round(b * 255.0)]
}

/// Every row but row 0 — the seam row kept literal — rotated sideways by one random
/// amount, and mirrored left-to-right half the time. A flip alone reads as a mirror, a
/// per-cell shuffle as a mosaic; a cyclic shift keeps every cell's exact neighbours, so
/// whatever gradient existed between them survives untouched.
fn rotated_below_seam(cells: &[Cell], cols: usize, rows: usize, seed: i32) -> Vec<Cell> {
    if rows <= 1 {
        return cells.to_vec();
    }
    let mut random = JavaRandom::new(seed);
    let mirror = random.next_boolean();
    let shift = random.next_int(cols);
    let mut out = cells.to_vec();
    for row in 1..rows {
        for x in 0..cols {
            let src = if mirror { cols - 1 - x } else { x };
            out[row * cols + x] = cells[row * cols + (src + shift) % cols];
        }
    }
    out
}

/// Flat at both ends, so no cell boundary shows up as a crease once magnified.
fn smoothstep(t: f32) -> f32 {
    let x = t.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn lerp_channel(from: u8, to: u8, t: f32) -> u8 {
    if t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return to;
    }
    (f32::from(from) + (f32::from(to) - f32::from(from)) * t).round() as u8
}

/// Smoothly resamples a cols×rows grid of control points up to a size×size texture,
/// which is what lets the stretch be plain bilinear: the creases are resampled away
/// before they are magnified.
fn resample_cells(cells: &[Cell], cols: usize, rows: usize, size: usize) -> Vec<u8> {
    let mut data = vec![0u8; size * size * 4];
    for ty in 0..size {
        let fy = ((ty as f32 + 0.5) / size as f32) * rows as f32 - 0.5;
        let y0 = (fy.floor() as usize).min(rows - 1);
        let y1 = (y0 + 1).min(rows - 1);
        let wy = smoothstep(fy - y0 as f32);
        for tx in 0..size {
            let fx = ((tx as f32 + 0.5) / size as f32) * cols as f32 - 0.5;
            let x0 = (fx.floor() as usize).min(cols - 1);
            let x1 = (x0 + 1).min(cols - 1);
            let wx = smoothstep(fx - x0 as f32);
            for (channel, out) in (0..3).zip(data[(ty * size + tx) * 4..(ty * size + tx) * 4 + 3].iter_mut()) {
                let top = lerp_channel(cells[y0 * cols + x0][channel], cells[y0 * cols + x1][channel], wx);
                let bottom = lerp_channel(cells[y1 * cols + x0][channel], cells[y1 * cols + x1][channel], wx);
                *out = lerp_channel(top, bottom, wy);
            }
            data[(ty * size + tx) * 4 + 3] = 255;
        }
    }
    data
}

/// Three small box passes approximate a broad Gaussian blur at thumbnail scale — a
/// sliding-window sum, so each pass is linear in the pixels it touches. In place on the
/// RGB bytes; alpha is left at 255.
fn box_blur(data: &mut [u8], width: usize, height: usize, passes: usize) {
    if width < 2 || height < 2 {
        return;
    }
    let radius = 2.max(width.min(height) / 12);
    let mut source = data.to_vec();
    let mut target = vec![0u8; source.len()];
    for _ in 0..passes {
        box_blur_pass(&source, &mut target, width, height, radius, true);
        box_blur_pass(&target, &mut source, width, height, radius, false);
    }
    data.copy_from_slice(&source);
}

fn box_blur_pass(source: &[u8], target: &mut [u8], width: usize, height: usize, radius: usize, horizontal: bool) {
    let (major, minor) = if horizontal { (width, height) } else { (height, width) };
    let window = (radius * 2 + 1) as f32;

    for fixed in 0..minor {
        // Signed: the running sum dips below zero a step at a time as the window moves.
        let (mut red, mut green, mut blue) = (0i64, 0i64, 0i64);
        let at = |offset: isize| -> usize {
            let position = offset.clamp(0, major as isize - 1) as usize;
            if horizontal { (fixed * width + position) * 3 } else { (position * width + fixed) * 3 }
        };
        for offset in -(radius as isize)..=(radius as isize) {
            let at = at(offset);
            red += i64::from(source[at]);
            green += i64::from(source[at + 1]);
            blue += i64::from(source[at + 2]);
        }
        for moving in 0..major {
            let index = if horizontal { (fixed * width + moving) * 3 } else { (moving * width + fixed) * 3 };
            // Truncated, as the reference's Uint8ClampedArray writes are (ToUint8Clamp floors).
            target[index] = (red as f32 / window) as u8;
            target[index + 1] = (green as f32 / window) as u8;
            target[index + 2] = (blue as f32 / window) as u8;
            let leaving = at(moving as isize - radius as isize);
            let entering = at(moving as isize + radius as isize + 1);
            red += i64::from(source[entering]) - i64::from(source[leaving]);
            green += i64::from(source[entering + 1]) - i64::from(source[leaving + 1]);
            blue += i64::from(source[entering + 2]) - i64::from(source[leaving + 2]);
        }
    }
}

/// The mesh for the artwork's pixels, end to end: mean per cell, the one liberty per
/// cell, the rotation (seeded by the artwork's URL, as the reference passes it), the
/// resample — then a touch of blur where the reference leans on the GPU's, and the same
/// decode box-blurred for the layer above it.
fn make_entry(ctx: &egui::Context, bytes: &[u8], url: &str) -> Option<Entry> {
    let img = image::load_from_memory(bytes).ok()?.to_rgb8();
    let img = image::imageops::resize(&img, MESH_PX, MESH_PX, image::imageops::FilterType::Triangle);
    let px: Vec<u8> = img.pixels().flat_map(|p| p.0).collect();

    let cols = MESH_GRID;
    let rows = MESH_GRID;
    let cells: Vec<Cell> = mesh_cells(&px, MESH_PX as usize, MESH_PX as usize).into_iter().map(lift_cell).collect();
    let rotated = rotated_below_seam(&cells, cols, rows, hash_url(url));
    let mesh = resample_cells(&rotated, cols, rows, MESH_TEX);
    let mut mesh = rgba(mesh, MESH_TEX);
    blur_mesh(&mut mesh, MESH_TEX, MESH_TEX);

    let mut blur = px;
    box_blur(&mut blur, MESH_PX as usize, MESH_PX as usize, FULL_BLUR_PASSES);
    Some(Entry {
        mesh: ctx.load_texture("np-mesh", egui::ColorImage::from_rgba_unmultiplied([MESH_TEX, MESH_TEX], &mesh), egui::TextureOptions::LINEAR),
        blur: ctx.load_texture("np-mesh-blur", egui::ColorImage::from_rgba_unmultiplied([MESH_PX as usize, MESH_PX as usize], &rgba(blur, MESH_PX as usize)), egui::TextureOptions::LINEAR),
    })
}

/// RGB bytes padded out to the RGBA egui reads.
fn rgba(rgb: Vec<u8>, size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size * size * 4);
    for at in (0..rgb.len()).step_by(3) {
        out.extend_from_slice(&[rgb[at], rgb[at + 1], rgb[at + 2], 255]);
    }
    out
}

/// A little softening on the 32×32 texture, standing in for the reference's 32 px CSS
/// blur once stretched: the resample already took the creases out.
fn blur_mesh(data: &mut [u8], width: usize, height: usize) {
    let radius = 2;
    let mut source = data.to_vec();
    let mut target = vec![0u8; source.len()];
    for _ in 0..2 {
        box_blur_pass_rgba(&source, &mut target, width, height, radius, true);
        box_blur_pass_rgba(&target, &mut source, width, height, radius, false);
    }
    data.copy_from_slice(&source);
}

/// Like [`box_blur_pass`], but over RGBA (the mesh texture carries alpha).
fn box_blur_pass_rgba(source: &[u8], target: &mut [u8], width: usize, height: usize, radius: usize, horizontal: bool) {
    let (major, minor) = if horizontal { (width, height) } else { (height, width) };
    let window = (radius * 2 + 1) as f32;

    for fixed in 0..minor {
        let mut rgb = [0i64; 3];
        let at = |offset: isize| -> usize {
            let position = offset.clamp(0, major as isize - 1) as usize;
            if horizontal { (fixed * width + position) * 4 } else { (position * width + fixed) * 4 }
        };
        for offset in -(radius as isize)..=(radius as isize) {
            let at = at(offset);
            for c in 0..3 {
                rgb[c] += i64::from(source[at + c]);
            }
        }
        for moving in 0..major {
            let index = if horizontal { (fixed * width + moving) * 4 } else { (moving * width + fixed) * 4 };
            for c in 0..3 {
                target[index + c] = (rgb[c] as f32 / window) as u8;
            }
            target[index + 3] = 255;
            let leaving = at(moving as isize - radius as isize);
            let entering = at(moving as isize + radius as isize + 1);
            for c in 0..3 {
                rgb[c] += i64::from(source[entering + c]) - i64::from(source[leaving + c]);
            }
        }
    }
}

/// The decoded cover's entry, from and cached across calls (one decode serves both
/// layers). `None` while it downloads or if it failed for good, and whether it was
/// already in the cache (a cached blur swaps in without a fade, like the reference's).
fn entry(ctx: &egui::Context, url: &str) -> Option<(Entry, bool)> {
    const CAP: usize = 64;
    let slot = egui::Id::unique("np-mesh-entries");
    let mut cache: HashMap<String, Slot> = ctx.data(|d| d.get_temp(slot)).unwrap_or_default();
    match cache.get(url) {
        Some(Slot::Ready(e)) => Some((e.clone(), true)),
        Some(Slot::Failed) | Some(Slot::Pending) => None,
        None => {
            let slot_value = match ctx.try_load_bytes(url) {
                Ok(egui::load::BytesPoll::Ready { bytes, .. }) => match make_entry(ctx, &bytes, url) {
                    Some(e) => Slot::Ready(e),
                    None => Slot::Failed,
                },
                Ok(egui::load::BytesPoll::Pending { .. }) => Slot::Pending,
                Err(_) => Slot::Failed,
            };
            if let Slot::Ready(e) = &slot_value {
                if cache.len() >= CAP {
                    cache.clear();
                }
                cache.insert(url.to_owned(), Slot::Ready(e.clone()));
                ctx.data_mut(|d| d.insert_temp(slot, cache));
                ctx.request_repaint();
                return Some((e.clone(), false));
            }
            let pending = matches!(slot_value, Slot::Pending);
            cache.insert(url.to_owned(), slot_value);
            ctx.data_mut(|d| d.insert_temp(slot, cache));
            if pending {
                ctx.request_repaint();
            }
            None
        }
    }
}

/// The now-playing backdrop over `rect`, for the cover in `thumbs`. `dim`: the preset's
/// np_dim as 0..1, fading both layers toward the dark base.
pub fn paint(ui: &Ui, rect: Rect, thumbs: &[Thumb], dim: f32) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, FALLBACK);
    let ctx = ui.ctx();
    let now = ctx.input(|i| i.time);
    let Some(t) = crate::model::pick_thumb(thumbs, MESH_PX) else { return };
    let url = crate::model::sized(&t.url, MESH_PX);

    let key = egui::Id::unique("np-mesh-shown");
    let mut shown: Option<Shown> = ctx.data(|d| d.get_temp(key));
    if let Some((entry, cached)) = entry(ctx, &url) {
        shown = Some(match shown {
            Some(mut s) if s.url == url => {
                // Back to what's on screen (a rebuilt texture after an eviction).
                s.mesh = entry.mesh;
                if let Some(tex) = &mut s.blur {
                    *tex = entry.blur;
                }
                s
            }
            Some(mut s) => {
                // The first colours show at once; later tracks cross-fade from
                // whatever is on screen. A blur that isn't cached yet fades the old
                // one out first; a cached one swaps in without a fade.
                s.from = Some((s.mesh.clone(), now));
                s.url = url.clone();
                s.mesh = entry.mesh;
                s.blur = Some(entry.blur);
                if !cached {
                    s.blur_up = false;
                    s.blur_since = now;
                } else if s.blur_up {
                    s.blur_since = f64::NEG_INFINITY;
                } else {
                    s.blur_up = true;
                    s.blur_since = now;
                }
                s
            }
            None => Shown { url: url.clone(), mesh: entry.mesh, blur: Some(entry.blur), blur_up: true, blur_since: f64::NEG_INFINITY, from: None },
        });
    }
    // The track's own decode, when one is in flight and its blur is on its way out.
    if shown.as_ref().is_some_and(|s| !s.blur_up)
        && let Some((entry, _)) = entry(ctx, &url)
        && let Some(s) = &mut shown
        && s.url == url
    {
        s.blur = Some(entry.blur);
        s.blur_up = true;
        s.blur_since = now;
    }

    let Some(mut s) = shown else {
        ctx.request_repaint();
        return;
    };
    let dim = 1.0 - dim.clamp(0.0, 1.0);

    // The mesh layer(s), a little oversize so their soft edge is off the view.
    let mesh_rect = rect.expand2(rect.size() * MESH_OVERSCAN);
    let mesh_scrim_top = Color32::from_black_alpha((0.06 * dim * 255.0) as u8);
    let mesh_scrim_bottom = Color32::from_black_alpha((0.30 * dim * 255.0) as u8);
    if let Some((old, at)) = &s.from {
        // The one before it stays fully up under the new one while it fades in.
        paint_mesh(&painter, mesh_rect, old, 1.0);
        scrim(&painter, mesh_rect, mesh_scrim_top, mesh_scrim_bottom, 1.0);
        let fade = (((now - at) / MESH_FADE) as f32).clamp(0.0, 1.0);
        // Ease out, so the old colours let go quickly and the new ones settle.
        let alpha = 1.0 - (1.0 - fade).powi(3);
        paint_mesh(&painter, mesh_rect, &s.mesh, alpha);
        scrim(&painter, mesh_rect, mesh_scrim_top, mesh_scrim_bottom, alpha);
        if fade < 1.0 {
            ctx.request_repaint();
        } else {
            s.from = None;
        }
    } else {
        paint_mesh(&painter, mesh_rect, &s.mesh, 1.0);
        scrim(&painter, mesh_rect, mesh_scrim_top, mesh_scrim_bottom, 1.0);
    }

    // The full-cover blur over its darker scrim: the layer the lyrics and queue live
    // on. It moves over 320 ms each way; a cached cover swaps without a fade.
    if let Some(blur) = &s.blur {
        let gone = (((now - s.blur_since) / FULL_BLUR_FADE) as f32).clamp(0.0, 1.0);
        let eased = gone * gone * (3.0 - 2.0 * gone);
        let alpha = if s.blur_up { eased } else { 1.0 - eased };
        let uv = cover_crop(rect);
        painter.image(blur.id(), rect, uv, Color32::WHITE.gamma_multiply(alpha * dim));
        scrim3(&painter, rect, dim, alpha);
        if gone < 1.0 {
            ctx.request_repaint();
        } else if !s.blur_up {
            // The old blur is all the way out: the mesh shows until the new decode.
            s.blur = None;
        }
    }

    ctx.data_mut(|d| d.insert_temp(key, s));
}

/// The square's sub-rect that fills a `rect` of any aspect, keeping the centre — the
/// reference's object-fit: cover.
fn cover_crop(rect: Rect) -> Rect {
    let aspect = rect.width() / rect.height().max(1.0);
    let (w, h) = if aspect >= 1.0 { (1.0, 1.0 / aspect) } else { (aspect, 1.0) };
    Rect::from_center_size(pos2(0.5, 0.5), vec2(w, h))
}

/// One stretched mesh texture at `alpha`.
fn paint_mesh(painter: &egui::Painter, rect: Rect, tex: &egui::TextureHandle, alpha: f32) {
    if alpha <= 0.003 {
        return;
    }
    painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE.gamma_multiply(alpha));
}

/// The mesh's light scrim, top to bottom, at `alpha`.
fn scrim(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32, alpha: f32) {
    if alpha <= 0.003 {
        return;
    }
    let top = top.gamma_multiply(alpha);
    let bottom = bottom.gamma_multiply(alpha);
    let mut mesh = egui::Mesh::default();
    let [tl, tr, bl, br] = [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()];
    for (pos, color) in [(tl, top), (tr, top), (bl, bottom), (br, bottom)] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 2, 1);
    mesh.add_triangle(1, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// The blur's darker legibility scrim: black 34% at the top, 48% at 55% down, 64% at the
/// bottom — as one gradient, at the layer's own `alpha` and the preset's `dim`.
fn scrim3(painter: &egui::Painter, rect: Rect, dim: f32, alpha: f32) {
    let a = alpha * dim;
    if a <= 0.003 {
        return;
    }
    let at = |v: f32| Color32::from_black_alpha((v * a * 255.0) as u8);
    let (top, mid, bottom) = (at(0.34), at(0.48), at(0.64));
    let seam = rect.top() + rect.height() * 0.55;
    let mut mesh = egui::Mesh::default();
    for (y, color) in [(rect.top(), top), (seam, mid), (rect.bottom(), bottom)] {
        mesh.colored_vertex(pos2(rect.left(), y), color);
        mesh.colored_vertex(pos2(rect.right(), y), color);
    }
    mesh.add_triangle(0, 2, 1);
    mesh.add_triangle(1, 2, 3);
    mesh.add_triangle(2, 4, 3);
    mesh.add_triangle(3, 4, 5);
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_hash_matches_java() {
        // Java: "abc".hashCode() == 96354, "a".hashCode() == 97.
        assert_eq!(hash_url("abc"), 96354);
        assert_eq!(hash_url("a"), 97);
        assert_eq!(hash_url(""), 0);
        // A value that overflows 32 bits along the way wraps, as Java's does.
        assert_eq!(hash_url("aaaaaa"), -1_425_372_064);
    }

    #[test]
    fn random_matches_java_util() {
        // new java.util.Random(42).nextInt(10) is 0, then 3; nextBoolean() true, false.
        let mut r = JavaRandom::new(42);
        assert_eq!(r.next_int(10), 0);
        assert_eq!(r.next_int(10), 3);
        let mut r = JavaRandom::new(42);
        assert!(r.next_boolean());
        assert!(!r.next_boolean());
        // A negative seed, like a URL hash that wraps: same on the JVM.
        let mut r = JavaRandom::new(-1);
        assert_eq!(r.next_int(100), 13);
    }

    #[test]
    fn cells_keep_proportions_and_flip() {
        // 2×2 pixels: the top half black, the bottom half red. Row 0 of the mesh is the
        // artwork's bottom edge — the red one.
        let px = vec![0, 0, 0, 0, 0, 0, 255, 0, 0, 255, 0, 0];
        let cells = mesh_cells(&px, 2, 2);
        assert_eq!(cells[0], [255, 0, 0]);
        assert_eq!(cells[1], [255, 0, 0]);
        assert_eq!(cells[2], [0, 0, 0]);
        assert_eq!(cells[3], [0, 0, 0]);
    }

    #[test]
    fn lift_never_lands_on_pure_black() {
        // The floor lifts black to 4.5% lightness, where the reference's own port lands.
        assert_eq!(lift_cell([0, 0, 0]), [11, 11, 11]);
        // A saturated colour keeps its hue, only nudged.
        assert_eq!(lift_cell([255, 0, 0]), [255, 0, 0]);
    }

    #[test]
    fn resample_is_flat_at_the_control_points() {
        // A 2×1 grid resampled to 4×4: reading back at the cell centres gives the cell,
        // and between them the same curve the reference's own port draws.
        let cells = vec![[10u8, 20, 30], [200, 210, 220]];
        let tex = resample_cells(&cells, 2, 1, 4);
        let at = |x: usize, y: usize| [tex[(y * 4 + x) * 4], tex[(y * 4 + x) * 4 + 1], tex[(y * 4 + x) * 4 + 2]];
        assert_eq!(at(0, 2), [10, 20, 30]);
        assert_eq!(at(1, 2), [40, 50, 60]);
        assert_eq!(at(2, 2), [170, 180, 190]);
        assert_eq!(at(3, 2), [200, 210, 220]);
    }

    #[test]
    fn the_quadrant_vector_matches_the_reference() {
        // A 120×120 cover of four quadrants (red, green top; blue, yellow bottom — the
        // image's bottom edge is row 0 of the mesh), read the same way the reference's
        // own TypeScript reads it, seed and all: same cells, same texture samples.
        let n = 120usize;
        let mut px = vec![0u8; n * n * 3];
        for y in 0..n {
            for x in 0..n {
                let c = match (y < 60, x < 60) {
                    (true, true) => [255u8, 0, 0],
                    (true, false) => [0, 255, 0],
                    (false, true) => [0, 0, 255],
                    (false, false) => [255, 255, 0],
                };
                let at = (y * n + x) * 3;
                px[at..at + 3].copy_from_slice(&c);
            }
        }
        let seed = hash_url("w120-h120");
        let cells: Vec<Cell> = mesh_cells(&px, n, n).into_iter().map(lift_cell).collect();
        let rotated = rotated_below_seam(&cells, 6, 6, seed);
        // Row 0 is the seam (the image's bottom edge, blue/yellow); the rows below it are
        // mirrored and shifted by the reference's own random draw for this seed; the
        // seam's colours are the top half's red/green.
        let (b, y, r, g) = ([0u8, 0, 255], [255u8, 255, 0], [255u8, 0, 0], [0u8, 255, 0]);
        let expected = vec![
            vec![b, b, b, y, y, y],
            vec![y, y, b, b, b, y],
            vec![y, y, b, b, b, y],
            vec![g, g, r, r, r, g],
            vec![g, g, r, r, r, g],
            vec![g, g, r, r, r, g],
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<Cell>>();
        assert_eq!(rotated, expected);
        let tex = resample_cells(&rotated, 6, 6, MESH_TEX);
        let at = |x: usize, y: usize| [tex[(y * MESH_TEX + x) * 4], tex[(y * MESH_TEX + x) * 4 + 1], tex[(y * MESH_TEX + x) * 4 + 2]];
        assert_eq!(at(0, 0), [0, 0, 255]);
        assert_eq!(at(16, 16), [163, 0, 92]);
        assert_eq!(at(8, 24), [6, 249, 0]);
    }

    #[test]
    fn blur_softens_a_lone_bright_pixel() {
        let mut px = vec![0u8; 24 * 24 * 3];
        let centre = (12 * 24 + 12) * 3;
        px[centre] = 255;
        px[centre + 1] = 255;
        px[centre + 2] = 255;
        box_blur(&mut px, 24, 24, 3);
        let at = |x: usize, y: usize| px[(y * 24 + x) * 3];
        // Radius max(2, 24/12) = 2, three passes: the pixel spreads, dimmed to a handful,
        // reaching a couple of pixels out and never past that or into the corner. The
        // numbers are the reference algorithm's own, at floor-writing semantics.
        assert_eq!(at(12, 12), 5);
        assert_eq!(at(10, 12), 4);
        assert_eq!(at(6, 12), 0);
        assert_eq!(at(0, 0), 0);
    }
}
