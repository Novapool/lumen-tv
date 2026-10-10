// Bakes the home screen's gradients and shadow into images, so the GPU draws
// one textured quad where it used to fill several blended layers. The Pi 5 GPU
// is fill-rate bound at 4K, so every layer costs. The maths copies Slint's
// femtovg renderer (gradient line from i_slint_core's line_for_angle, radial
// radius = half the box diagonal, shadow sigma = blur / 2), so the images match
// what Slint drew live.
//
// - Hero backdrop (app gradient at 55% + radial and bottom fades): baked small
//   (1/3 of 1080p) and stretched, since it is soft gradients only. Small also
//   matters because Slint drops a texture as soon as nothing shows it, so each
//   focus change uploads a backdrop again (at 1920 wide that alone made 10% of
//   frames slow at 1080p). Crossfades are blended here on the CPU (blend), so
//   the GPU never draws two full-width layers.
// - Tile faces (gradient + gloss + rounded corners): baked at the focused size
//   in physical pixels, since their edges must stay sharp at 4K.
// - Tile shadow: one image shared by every tile, baked at 1/4 resolution.

use crate::icons::{self, Icon};
use slint::{Image, Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer};

const HERO_W: u32 = 640;
const HERO_H: u32 = 234; // 65% of 360
const HERO_OPACITY: f32 = 0.55;
const BG: [f32; 3] = [5., 6., 10.]; // Theme.bg #05060a

/// Tile geometry in rem, as in tile.slint.
const TILE_W: f32 = 19.;
const TILE_H: f32 = 11.4;
const TILE_RADIUS: f32 = 1.1;
const TILE_FOCUS: f32 = 1.12;
const SHADOW_BLUR: f32 = 3.2 * TILE_FOCUS + 1.2; // +1.2rem makes up for the resting-size box
const SHADOW_ALPHA: f32 = 0.65;
const SHADOW_SCALE: f32 = 0.25;

/// The hero backdrop for an app's tile gradient.
pub fn hero(bg: &Tint) -> Image {
    let (w, h) = (HERO_W as f32, HERO_H as f32);
    let ramp = Ramp::new(bg, w, h);

    // radial fade: circle at (70%, 0) in a 2r square, so its radius is r * sqrt(2)
    let (cx, r) = (0.7 * w, (0.7 * w).hypot(h));
    let radius = r * std::f32::consts::SQRT_2;

    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(HERO_W, HERO_H);
    let px = buf.make_mut_slice();
    for y in 0..HERO_H {
        for x in 0..HERO_W {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut c = BG;
            over(&mut c, ramp.at(fx, fy), HERO_OPACITY);
            over(&mut c, BG, fade((fx - cx).hypot(fy) / radius, 0.3, 0.85));
            over(&mut c, BG, fade(fy / h, 0.4, 1.0));
            px[(y * HERO_W + x) as usize] = Rgb8Pixel::new(byte(c[0]), byte(c[1]), byte(c[2]));
        }
    }
    Image::from_rgb8(buf)
}

/// The crossfade frame between two baked backdrops: `w` is b's share.
/// At either end it returns the baked image itself, so its GPU texture is reused.
pub fn blend(a: Image, b: Image, w: f32) -> Image {
    let (Some(pa), Some(pb)) = (a.to_rgb8(), b.to_rgb8()) else { return b };
    if w <= 0.002 || pa.width() != pb.width() || pa.height() != pb.height() {
        return if w < 0.5 { a } else { b };
    }
    if w >= 0.998 {
        return b;
    }
    let mut out = SharedPixelBuffer::<Rgb8Pixel>::new(pa.width(), pa.height());
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * w).round() as u8;
    for ((o, x), y) in out.make_mut_slice().iter_mut().zip(pa.as_slice()).zip(pb.as_slice()) {
        *o = Rgb8Pixel::new(mix(x.r, y.r), mix(x.g, y.g), mix(x.b, y.b));
    }
    Image::from_rgb8(out)
}

/// Fraction of the face's height / width an icon may fill.
const ICON_MAX_H: f32 = 0.55;
const ICON_MAX_W: f32 = 0.70;

/// Where an icon of natural size `natural` lands on a w x h face: (x, y, w, h)
/// in pixels, centred, aspect kept, within ICON_MAX_W / ICON_MAX_H.
pub fn icon_box(w: f32, h: f32, natural: (f32, f32)) -> (u32, u32, u32, u32) {
    let k = (w * ICON_MAX_W / natural.0).min(h * ICON_MAX_H / natural.1);
    let (tw, th) = ((natural.0 * k).round().max(1.), (natural.1 * k).round().max(1.));
    (((w - tw) / 2.).round() as u32, ((h - th) / 2.).round() as u32, tw as u32, th as u32)
}

/// A tile face at its focused size for `rem` physical pixels: the app brush,
/// the icon (if any, over the gradient and under the gloss), the top gloss and
/// antialiased rounded corners (premultiplied alpha).
pub fn tile_face(bg: &Tint, rem: f32, icon: Option<&Icon>) -> Image {
    let (w, h) = ((TILE_W * TILE_FOCUS * rem).ceil(), (TILE_H * TILE_FOCUS * rem).ceil());
    let radius = TILE_RADIUS * TILE_FOCUS * rem;
    let ramp = Ramp::new(bg, w, h);
    // linear-gradient(180deg, #ffffff33 0%, #ffffff0a 45%, #ffffff00 55%)
    let gloss = [(0., 0x33 as f32 / 255.), (0.45, 0x0a as f32 / 255.), (0.55, 0.)];

    let (iw, ih) = (w as u32, h as u32);
    // the icon, rendered once at its final pixel size (premultiplied RGBA)
    let icon_px = icon.map(|i| {
        let (bx, by, bw, bh) = icon_box(w, h, i.size());
        (bx, by, bw, bh, icons::render(i, bw, bh))
    });
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(iw, ih);
    let px = buf.make_mut_slice();
    for y in 0..ih {
        for x in 0..iw {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut c = ramp.at(fx, fy);
            if let Some((bx, by, bw, bh, ip)) = &icon_px {
                if x >= *bx && y >= *by && x < bx + bw && y < by + bh {
                    let i = (((y - by) * bw + (x - bx)) * 4) as usize;
                    let k = 1. - ip[i + 3] as f32 / 255.;
                    for ch in 0..3 {
                        c[ch] = (c[ch] * k + ip[i + ch] as f32).min(255.);
                    }
                }
            }
            over(&mut c, [255.; 3], stops_at(&gloss, fy / h));
            let a = coverage(fx, fy, w, h, radius);
            px[(y * iw + x) as usize] =
                Rgba8Pixel::new(byte(c[0] * a), byte(c[1] * a), byte(c[2] * a), byte(255. * a));
        }
    }
    Image::from_rgba8_premultiplied(buf)
}

/// Geometry of the shared tile shadow in rem, relative to the tile's top left.
fn shadow_rect() -> (f32, f32, f32, f32) {
    let offset_y = 2.2 * TILE_FOCUS + 0.5;
    (-SHADOW_BLUR, -SHADOW_BLUR + offset_y, TILE_W + 2. * SHADOW_BLUR, TILE_H + 2. * SHADOW_BLUR)
}

/// The focus shadow (black at 65%, Gaussian blur, sigma = blur / 2) for a
/// resting-size tile, baked at 1/4 resolution: it is a blur, so it stretches well.
pub fn tile_shadow(rem: f32) -> Image {
    let k = rem * SHADOW_SCALE; // image pixels per rem
    let (_, _, w, h) = shadow_rect();
    let (iw, ih) = ((w * k).ceil() as usize, (h * k).ceil() as usize);
    let (bx, by) = (SHADOW_BLUR * k, SHADOW_BLUR * k);
    let (tw, th, tr) = (TILE_W * k, TILE_H * k, TILE_RADIUS * k);

    let mut mask: Vec<f32> = (0..iw * ih)
        .map(|i| {
            let (x, y) = ((i % iw) as f32 + 0.5 - bx, (i / iw) as f32 + 0.5 - by);
            coverage(x, y, tw, th, tr)
        })
        .collect();
    gaussian_blur(&mut mask, iw, ih, SHADOW_BLUR * k / 2.);

    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(iw as u32, ih as u32);
    for (p, m) in buf.make_mut_slice().iter_mut().zip(&mask) {
        *p = Rgba8Pixel::new(0, 0, 0, byte(255. * SHADOW_ALPHA * m));
    }
    Image::from_rgba8_premultiplied(buf)
}

/// Separable Gaussian blur of a single-channel image, edges clamped to zero.
fn gaussian_blur(img: &mut [f32], w: usize, h: usize, sigma: f32) {
    let r = (sigma * 3.).ceil() as isize;
    let kernel: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2. * sigma * sigma)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|v| v / sum).collect();
    let mut tmp = vec![0.; img.len()];
    let pass = |src: &[f32], dst: &mut [f32], step: (isize, isize)| {
        for y in 0..h as isize {
            for x in 0..w as isize {
                let mut acc = 0.;
                for (j, kv) in kernel.iter().enumerate() {
                    let d = j as isize - r;
                    let (sx, sy) = (x + d * step.0, y + d * step.1);
                    if sx >= 0 && sy >= 0 && sx < w as isize && sy < h as isize {
                        acc += kv * src[(sy * w as isize + sx) as usize];
                    }
                }
                dst[(y * w as isize + x) as usize] = acc;
            }
        }
    };
    pass(img, &mut tmp, (1, 0));
    pass(&tmp, img, (0, 1));
}

/// How much of the pixel centred at (x, y) lies inside a w x h rounded rect at
/// the origin (1px antialiasing from the signed distance).
fn coverage(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (qx, qy) = ((x - w / 2.).abs() - (w / 2. - r), (y - h / 2.).abs() - (h / 2. - r));
    let outside = qx.max(0.).hypot(qy.max(0.));
    let d = outside + qx.max(qy).min(0.) - r;
    (0.5 - d).clamp(0., 1.)
}

fn over(c: &mut [f32; 3], top: [f32; 3], a: f32) {
    for i in 0..3 {
        c[i] = (c[i] * (1. - a) + top[i] * a).clamp(0., 255.);
    }
}

fn byte(v: f32) -> u8 {
    v.round().clamp(0., 255.) as u8
}

/// Alpha of a fade from transparent at `t0` to opaque at `t1` (linear, like femtovg).
fn fade(t: f32, t0: f32, t1: f32) -> f32 {
    ((t - t0) / (t1 - t0)).clamp(0., 1.)
}

/// Piecewise-linear value of (position, value) stops at t, clamped at the ends.
fn stops_at(stops: &[(f32, f32)], t: f32) -> f32 {
    let i = stops.partition_point(|s| s.0 < t);
    if i == 0 {
        return stops[0].1;
    }
    if i == stops.len() {
        return stops[i - 1].1;
    }
    let ((p0, v0), (p1, v1)) = (stops[i - 1], stops[i]);
    v0 + (v1 - v0) * (t - p0) / (p1 - p0).max(f32::EPSILON)
}

/// A Slint linear gradient laid over a w x h box.
struct Ramp {
    start: (f32, f32),
    dir: (f32, f32), // (end - start) / |end - start|^2
    stops: Vec<(f32, [f32; 3])>,
}

/// A tile's colour gradient: an angle and colour stops (positions 0..1).
/// Kept in Rust beside the icon; Slint never draws it, only the bakes do.
#[derive(Clone, Debug, PartialEq)]
pub struct Tint {
    angle: f32,
    stops: Vec<(f32, [f32; 3])>,
}

impl Tint {
    /// `colors` (rgb 0..255) spread evenly along a gradient at `angle` degrees.
    pub fn from_stops(angle: f32, colors: &[[u8; 3]]) -> Tint {
        let last = (colors.len().max(2) - 1) as f32;
        let stops = colors.iter().enumerate().map(|(i, c)| (i as f32 / last, c.map(f32::from))).collect();
        Tint { angle, stops }
    }
}

impl Ramp {
    fn new(tint: &Tint, w: f32, h: f32) -> Ramp {
        let (start, end) = line_for_angle(tint.angle, w, h);
        let (dx, dy) = (end.0 - start.0, end.1 - start.1);
        let len2 = (dx * dx + dy * dy).max(f32::EPSILON);
        Ramp { start, dir: (dx / len2, dy / len2), stops: tint.stops.clone() }
    }

    fn at(&self, x: f32, y: f32) -> [f32; 3] {
        let t = (x - self.start.0) * self.dir.0 + (y - self.start.1) * self.dir.1;
        let i = self.stops.partition_point(|s| s.0 < t);
        if i == 0 {
            return self.stops[0].1;
        }
        if i == self.stops.len() {
            return self.stops[i - 1].1;
        }
        let ((p0, c0), (p1, c1)) = (self.stops[i - 1], self.stops[i]);
        let f = (t - p0) / (p1 - p0).max(f32::EPSILON);
        [0, 1, 2].map(|k| c0[k] + (c1[k] - c0[k]) * f)
    }
}

/// i_slint_core::graphics::line_for_angle (not public API), ported as-is.
fn line_for_angle(angle: f32, w: f32, h: f32) -> ((f32, f32), (f32, f32)) {
    let (s, c) = (angle + 90.).to_radians().sin_cos();
    let (a, b) = if s.abs() < f32::EPSILON {
        let y = h / 2.;
        return if c < 0. { ((0., y), (w, y)) } else { ((w, y), (0., y)) };
    } else if c * s < 0. {
        let x = (s * w + c * h) * s / 2.;
        let y = -c * x / s + h;
        ((x, y), (w - x, h - y))
    } else {
        let x = (s * w - c * h) * s / 2.;
        let y = -c * x / s;
        ((w - x, h - y), (x, y))
    };
    if s > 0. { (a, b) } else { (b, a) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::tests::{png, tmp, SVG};

    fn face_px(icon: Option<&Icon>) -> (u32, u32, Vec<u8>) {
        let img = tile_face(&Tint::from_stops(135., &[[40, 90, 160], [90, 40, 160]]), 8., icon);
        let b = img.to_rgba8_premultiplied().unwrap();
        (b.width(), b.height(), b.as_bytes().to_vec())
    }

    #[test]
    fn icon_changes_only_its_box() {
        let d = tmp("bake");
        std::fs::write(d.join("a.svg"), SVG).unwrap();
        let icon = icons::load(&d.join("a.svg")).unwrap();
        let (w, h, plain) = face_px(None);
        let (_, _, with) = face_px(Some(&icon));
        let (bx, by, bw, bh) = icon_box(w as f32, h as f32, icon.size());
        assert!(bw as f32 <= w as f32 * ICON_MAX_W + 1. && bh as f32 <= h as f32 * ICON_MAX_H + 1.);
        let (mut inside, mut outside) = (0, 0);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                if plain[i..i + 4] != with[i..i + 4] {
                    if x >= bx && y >= by && x < bx + bw && y < by + bh {
                        inside += 1;
                    } else {
                        outside += 1;
                    }
                }
            }
        }
        assert!(inside > 0, "icon not drawn");
        assert_eq!(outside, 0, "icon leaked outside its box");
        assert!((bx as i32 * 2 + bw as i32 - w as i32).abs() <= 1);
        assert!((by as i32 * 2 + bh as i32 - h as i32).abs() <= 1);
    }

    #[test]
    fn png_icon_draws_and_corners_stay_clear() {
        let d = tmp("bakepng");
        std::fs::write(d.join("a.png"), png(256, 256, [255, 255, 255, 255])).unwrap();
        let icon = icons::load(&d.join("a.png")).unwrap();
        let (w, h, with) = face_px(Some(&icon));
        assert_eq!(&with[..4], &[0, 0, 0, 0]); // top-left corner still transparent
        let c = (((h / 2) * w + w / 2) * 4) as usize;
        assert!(with[c] > 200 && with[c + 3] == 255, "centre should be the white icon");
    }

    /// Bake-time impact at 32 px/rem: `cargo test --release -- --ignored --nocapture bake_timing`.
    #[test]
    #[ignore]
    fn bake_timing() {
        let d = tmp("timing");
        std::fs::write(d.join("a.png"), png(256, 256, [255, 255, 255, 255])).unwrap();
        std::fs::write(d.join("a.svg"), SVG).unwrap();
        let (p, s) = (icons::load(&d.join("a.png")).unwrap(), icons::load(&d.join("a.svg")).unwrap());
        let brush = Tint::from_stops(135., &[[40, 90, 160], [90, 40, 160]]);
        let time = |label: &str, icon: Option<&Icon>| {
            let _ = tile_face(&brush, 32., icon);
            let t = std::time::Instant::now();
            for _ in 0..5 {
                let _ = tile_face(&brush, 32., icon);
            }
            println!("TIMING {label}: {:.1} ms/face", t.elapsed().as_secs_f64() * 200.);
        };
        time("no icon", None);
        time("png 256 icon", Some(&p));
        time("svg icon", Some(&s));
    }
}

