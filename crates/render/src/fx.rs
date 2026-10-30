//! Raster effects on premultiplied RGBA8 pixmaps: blur (3× box ≈ Gaussian), dilate/erode (spread),
//! alpha extraction and tinting.

use std::collections::VecDeque;
use tiny_skia::Pixmap;

/// Box radii for three passes approximating a Gaussian of `sigma` (Kovesi).
fn box_sizes(sigma: f64) -> [usize; 3] {
    let n = 3.0;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma - n * (wl * wl) as f64 - 4.0 * n * wl as f64 - 3.0 * n) / (-4.0 * wl as f64 - 4.0);
    let m = m_ideal.round() as i64;
    let r = |i: i64| -> usize {
        let w = if i < m { wl } else { wu };
        ((w.max(1) - 1) / 2) as usize
    };
    [r(0), r(1), r(2)]
}

fn box_blur_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let div = (2 * r + 1) as u32;
    for y in 0..h {
        if crate::render_interrupted() {
            return;
        }
        let row = y * w * 4;
        let mut acc = [0u32; 4];
        // Edge: transparent outside.
        for x in 0..=r.min(w.saturating_sub(1)) {
            for c in 0..4 {
                acc[c] += u32::from(src.get(row + x * 4 + c).copied().unwrap_or(0));
            }
        }
        for x in 0..w {
            for c in 0..4 {
                if let Some(d) = dst.get_mut(row + x * 4 + c) {
                    *d = (acc[c] / div) as u8;
                }
            }
            let add = x + r + 1;
            if add < w {
                for c in 0..4 {
                    acc[c] += u32::from(src.get(row + add * 4 + c).copied().unwrap_or(0));
                }
            }
            if x >= r {
                let sub = x - r;
                for c in 0..4 {
                    acc[c] = acc[c].saturating_sub(u32::from(src.get(row + sub * 4 + c).copied().unwrap_or(0)));
                }
            }
        }
    }
}

fn transpose(src: &[u8], dst: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        if crate::render_interrupted() {
            return;
        }
        for x in 0..w {
            let s = (y * w + x) * 4;
            let d = (x * h + y) * 4;
            if let (Some(sp), Some(dp)) = (src.get(s..s + 4), dst.get_mut(d..d + 4)) {
                dp.copy_from_slice(sp);
            }
        }
    }
}

/// The largest blur standard deviation drawn (pixels): far wider than any layer (at most 16,384
/// pixels a side), so wider blurs look the same (everything averages out to nothing).
pub const MAX_SIGMA: f64 = 1_000_000.0;

/// Average whole source cells. Shadow masks are premultiplied, so averaging their bytes preserves
/// coverage without the ringing and large temporary buffers of a general image resampler.
fn reduce_mask(src: &Pixmap, dst: &mut Pixmap, factor: u32) {
    let (w, h, sw, sh) = (src.width() as usize, src.height() as usize, dst.width() as usize, dst.height() as usize);
    let factor = factor as usize;
    let input = src.data();
    let output = dst.data_mut();
    for y in 0..sh {
        if crate::render_interrupted() {
            return;
        }
        let y0 = y * factor;
        let y1 = (y0 + factor).min(h);
        for x in 0..sw {
            let x0 = x * factor;
            let x1 = (x0 + factor).min(w);
            let mut sums = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * w + sx) * 4;
                    for c in 0..4 {
                        sums[c] += u32::from(input[i + c]);
                    }
                }
            }
            let count = ((x1 - x0) * (y1 - y0)) as u32;
            let i = (y * sw + x) * 4;
            for c in 0..4 {
                output[i + c] = (sums[c] / count) as u8;
            }
        }
    }
}

/// Bilinear reconstruction at pixel centres, with edge pixels extended. Only the soft effect is
/// enlarged; the layer's vector geometry continues to render at its requested device scale.
fn expand_mask(src: &Pixmap, dst: &mut Pixmap, factor: u32, origin: (u32, u32)) {
    let (w, h, sw, sh) = (dst.width() as usize, dst.height() as usize, src.width() as usize, src.height() as usize);
    let factor = factor as usize;
    let input = src.data();
    let output = dst.data_mut();
    let xs: Vec<_> = (0..w)
        .map(|x| {
            let x = x + origin.0 as usize;
            let position = (2 * x + 1).saturating_sub(factor);
            let left = position / (2 * factor);
            let right = (left + 1).min(sw - 1);
            let fx = if 2 * x + 1 < factor { 0 } else { position % (2 * factor) * 256 / (2 * factor) };
            (left.min(sw - 1), right, fx as u32)
        })
        .collect();
    for y in 0..h {
        if crate::render_interrupted() {
            return;
        }
        let y = y + origin.1 as usize;
        let position = (2 * y + 1).saturating_sub(factor);
        let top = position / (2 * factor);
        let bottom = (top + 1).min(sh - 1);
        let fy = if 2 * y + 1 < factor { 0 } else { (position % (2 * factor) * 256 / (2 * factor)) as u32 };
        let top = top.min(sh - 1);
        for (x, &(left, right, fx)) in xs.iter().enumerate() {
            let (a, b, c, d) = ((top * sw + left) * 4, (top * sw + right) * 4, (bottom * sw + left) * 4, (bottom * sw + right) * 4);
            let out = ((y - origin.1 as usize) * w + x) * 4;
            for channel in 0..4 {
                let upper = u32::from(input[a + channel]) * (256 - fx) + u32::from(input[b + channel]) * fx;
                let lower = u32::from(input[c + channel]) * (256 - fx) + u32::from(input[d + channel]) * fx;
                output[out + channel] = ((upper * (256 - fy) + lower * fy + 32_768) >> 16) as u8;
            }
        }
    }
}

/// Gaussian-like blur of the whole pixmap with standard deviation `sigma` (pixels).
pub fn blur(p: &mut Pixmap, sigma: f64) {
    if !(sigma.is_finite() && sigma > 0.3) {
        return;
    }
    let sigma = sigma.min(MAX_SIGMA);
    // Broad shadows have no detail at their blur scale. Blur the premultiplied colour buffer at
    // a proportional resolution, then interpolate it. The underlying vector paint and sharp
    // mask are still rasterized at the full device resolution.
    let factor = if sigma >= 192.0 {
        8
    } else if sigma >= 96.0 {
        4
    } else if sigma >= 48.0 {
        2
    } else {
        1
    };
    if factor > 1 && p.width() >= factor * 16 && p.height() >= factor * 16 {
        let (w, h) = (p.width(), p.height());
        let (sw, sh) = (w.div_ceil(factor), h.div_ceil(factor));
        if let Some(mut reduced) = Pixmap::new(sw, sh) {
            reduce_mask(p, &mut reduced, factor);
            blur_exact(&mut reduced, sigma / f64::from(factor));
            expand_mask(&reduced, p, factor, (0, 0));
            return;
        }
    }
    blur_exact(p, sigma);
}

/// Blur a padded mask and reconstruct only its visible centre. Inner shadows need surrounding
/// pixels to calculate the blur, but painting those pixels at full resolution wastes most work.
pub fn blur_cropped(mut source: Pixmap, sigma: f64, origin: (u32, u32), size: (u32, u32)) -> Option<Pixmap> {
    if origin.0.checked_add(size.0)? > source.width() || origin.1.checked_add(size.1)? > source.height() {
        return None;
    }
    let mut result = Pixmap::new(size.0, size.1)?;
    let factor = if sigma >= 192.0 {
        8
    } else if sigma >= 96.0 {
        4
    } else if sigma >= 48.0 {
        2
    } else {
        1
    };
    if factor > 1 && source.width() >= factor * 16 && source.height() >= factor * 16 {
        let mut reduced = Pixmap::new(source.width().div_ceil(factor), source.height().div_ceil(factor))?;
        reduce_mask(&source, &mut reduced, factor);
        blur_exact(&mut reduced, sigma.min(MAX_SIGMA) / f64::from(factor));
        expand_mask(&reduced, &mut result, factor, origin);
    } else {
        if sigma.is_finite() && sigma > 0.3 {
            blur_exact(&mut source, sigma.min(MAX_SIGMA));
        }
        for row in 0..size.1 as usize {
            let from = ((origin.1 as usize + row) * source.width() as usize + origin.0 as usize) * 4;
            let to = row * size.0 as usize * 4;
            result.data_mut()[to..to + size.0 as usize * 4].copy_from_slice(&source.data()[from..from + size.0 as usize * 4]);
        }
    }
    Some(result)
}

fn blur_exact(p: &mut Pixmap, sigma: f64) {
    let (w, h) = (p.width() as usize, p.height() as usize);
    let radii = box_sizes(sigma);
    let mut a = p.data().to_vec();
    let mut b = vec![0u8; a.len()];
    for r in radii {
        box_blur_h(&a, &mut b, w, h, r);
        std::mem::swap(&mut a, &mut b);
    }
    transpose(&a, &mut b, w, h);
    std::mem::swap(&mut a, &mut b);
    for r in radii {
        box_blur_h(&a, &mut b, h, w, r);
        std::mem::swap(&mut a, &mut b);
    }
    transpose(&a, &mut b, h, w);
    p.data_mut().copy_from_slice(&b);
}

/// Grow (`r > 0`) or shrink (`r < 0`) the alpha of a pixmap by `r` pixels (square structuring
/// element, separable max/min). Colour channels follow alpha (tinted afterwards anyway).
pub fn morph(p: &mut Pixmap, r: i32) {
    if r == 0 {
        return;
    }
    let grow = r > 0;
    let r = r.unsigned_abs().min(400) as usize;
    let (w, h) = (p.width() as usize, p.height() as usize);
    let alpha: Vec<u8> = p.data().as_chunks::<4>().0.iter().map(|c| c[3]).collect();
    let mut a1 = vec![0u8; alpha.len()];
    let mut a2 = vec![0u8; alpha.len()];
    // A monotonic queue keeps the maximum or minimum of each moving window. Spread is therefore
    // O(width × height), even when a large zoom turns a small document radius into hundreds of
    // device pixels.
    let mut window = VecDeque::new();
    for y in 0..h {
        if crate::render_interrupted() {
            return;
        }
        window.clear();
        let mut next = 0;
        for x in 0..w {
            let hi = x.saturating_add(r).min(w - 1);
            while next <= hi {
                let value = alpha[y * w + next];
                while let Some(&back) = window.back() {
                    let previous = alpha[y * w + back];
                    if (grow && previous <= value) || (!grow && previous >= value) {
                        window.pop_back();
                    } else {
                        break;
                    }
                }
                window.push_back(next);
                next += 1;
            }
            let lo = x.saturating_sub(r);
            while window.front().is_some_and(|&front| front < lo) {
                window.pop_front();
            }
            a1[y * w + x] =
                if !grow && (x < r || x.saturating_add(r) >= w) { 0 } else { window.front().map(|&front| alpha[y * w + front]).unwrap_or(0) };
        }
    }
    for x in 0..w {
        if crate::render_interrupted() {
            return;
        }
        window.clear();
        let mut next = 0;
        for y in 0..h {
            let hi = y.saturating_add(r).min(h - 1);
            while next <= hi {
                let value = a1[next * w + x];
                while let Some(&back) = window.back() {
                    let previous = a1[back * w + x];
                    if (grow && previous <= value) || (!grow && previous >= value) {
                        window.pop_back();
                    } else {
                        break;
                    }
                }
                window.push_back(next);
                next += 1;
            }
            let lo = y.saturating_sub(r);
            while window.front().is_some_and(|&front| front < lo) {
                window.pop_front();
            }
            a2[y * w + x] =
                if !grow && (y < r || y.saturating_add(r) >= h) { 0 } else { window.front().map(|&front| a1[front * w + x]).unwrap_or(0) };
        }
    }
    for (px, a) in p.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(a2) {
        px.copy_from_slice(&[a, a, a, a]);
    }
}

/// Replace every pixel with `color` (non-premultiplied RGBA 0..1) at the pixel's alpha × color alpha.
pub fn tint(p: &mut Pixmap, color: [f32; 4]) {
    let [r, g, b, a] = color;
    for px in p.data_mut().as_chunks_mut::<4>().0 {
        let alpha = f32::from(px.get(3).copied().unwrap_or(0)) / 255.0 * a;
        let q = |c: f32| (c * alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        px.copy_from_slice(&[q(r), q(g), q(b), (alpha * 255.0).round().clamp(0.0, 255.0) as u8]);
    }
}

/// Invert the alpha (premultiplied white with alpha 1-a): used for inner shadows.
pub fn invert_alpha(p: &mut Pixmap) {
    for px in p.data_mut().as_chunks_mut::<4>().0 {
        let a = 255 - px.get(3).copied().unwrap_or(0);
        px.copy_from_slice(&[a, a, a, a]);
    }
}

/// Multiply every pixel of `p` by the alpha of `m` (same size); `invert` uses 1-alpha.
pub fn mask_by(p: &mut Pixmap, m: &Pixmap, invert: bool) {
    for (px, mp) in p.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(m.data().chunks_exact(4)) {
        let ma = u32::from(mp.get(3).copied().unwrap_or(0));
        let ma = if invert { 255 - ma } else { ma };
        for c in px.iter_mut() {
            *c = ((u32::from(*c) * ma + 127) / 255) as u8;
        }
    }
}

/// Call `f(dst_row, src_row, row, first_col)` for each row where `src`, with its top-left at `at`
/// on `dst`, overlaps it (premultiplied RGBA bytes of the overlapping columns; `row` and
/// `first_col` in `src`'s pixels). Nothing past either edge is visited.
pub(crate) fn overlap_rows(dst: &mut Pixmap, at: (i32, i32), src: &Pixmap, mut f: impl FnMut(&mut [u8], &[u8], i64, i64)) {
    let (dw, dh) = (i64::from(dst.width()), i64::from(dst.height()));
    let (sw, sh) = (i64::from(src.width()), i64::from(src.height()));
    let (ax, ay) = (i64::from(at.0), i64::from(at.1));
    let (c0, c1) = ((-ax).max(0), sw.min(dw - ax));
    let (r0, r1) = ((-ay).max(0), sh.min(dh - ay));
    if c1 <= c0 || r1 <= r0 {
        return;
    }
    let len = (c1 - c0) as usize * 4;
    let s = src.data();
    let d = dst.data_mut();
    for r in r0..r1 {
        let (si, di) = ((r * sw + c0) as usize * 4, ((r + ay) * dw + ax + c0) as usize * 4);
        if let (Some(sp), Some(dp)) = (s.get(si..si + len), d.get_mut(di..di + len)) {
            f(dp, sp, r, c0);
        }
    }
}

/// Composite `src` at `opacity` onto `dst` (top-left at `at`) with plus-darker (linear burn:
/// `max(0, backdrop + source - 1)`, through the W3C general compositing formula), which
/// tiny-skia lacks.
pub fn plus_darker(dst: &mut Pixmap, at: (i32, i32), src: &Pixmap, opacity: f32) {
    let o = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 0.0 };
    overlap_rows(dst, at, src, |dp, sp, _, _| {
        for (d, s) in dp.chunks_exact_mut(4).zip(sp.chunks_exact(4)) {
            let f = |v: u8| f32::from(v) / 255.0;
            let sa = f(s[3]) * o;
            if sa <= 0.0 {
                continue;
            }
            let da = f(d[3]);
            for c in 0..3 {
                let (sc, dc) = (f(s[c]) * o, f(d[c]));
                let v = sc * (1.0 - da) + dc * (1.0 - sa) + (sa * dc + da * sc - sa * da).max(0.0);
                d[c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            d[3] = ((sa + da - sa * da).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_spreads_and_conserves() {
        let mut p = Pixmap::new(64, 64).unwrap();
        let i = (32 * 64 + 32) * 4;
        p.data_mut()[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
        let before: u32 = p.data().iter().map(|v| u32::from(*v)).sum();
        blur(&mut p, 3.0);
        let after: u32 = p.data().iter().map(|v| u32::from(*v)).sum();
        assert!(p.data()[i + 3] < 255);
        // Integer rounding loses a little.
        assert!(after <= before && after > 0, "{before} {after}");
    }

    #[test]
    fn morph_grows() {
        let mut p = Pixmap::new(20, 20).unwrap();
        let i = (10 * 20 + 10) * 4;
        p.data_mut()[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
        morph(&mut p, 2);
        let count = p.data().as_chunks::<4>().0.iter().filter(|c| c[3] == 255).count();
        assert_eq!(count, 25);
        morph(&mut p, -2);
        let count = p.data().as_chunks::<4>().0.iter().filter(|c| c[3] == 255).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn morph_matches_window_reference_at_edges_and_large_radii() {
        for (w, h) in [(1, 1), (2, 7), (7, 2), (9, 11)] {
            let mut original = Pixmap::new(w, h).unwrap();
            for (i, px) in original.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
                px[3] = ((i * 73 + 19) % 256) as u8;
            }
            for radius in [-20, -5, -1, 1, 3, 20] {
                let mut actual = original.clone();
                morph(&mut actual, radius);
                let r = radius.unsigned_abs() as i32;
                let grow = radius > 0;
                let mut horizontal = vec![0u8; (w * h) as usize];
                for y in 0..h as i32 {
                    for x in 0..w as i32 {
                        horizontal[(y as u32 * w + x as u32) as usize] = if !grow && (x < r || x + r >= w as i32) {
                            0
                        } else {
                            ((x - r).max(0)..=(x + r).min(w as i32 - 1))
                                .map(|sx| original.data()[((y as u32 * w + sx as u32) * 4 + 3) as usize])
                                .fold(if grow { 0 } else { 255 }, |a, b| if grow { a.max(b) } else { a.min(b) })
                        };
                    }
                }
                for y in 0..h as i32 {
                    for x in 0..w as i32 {
                        let expected = if !grow && (y < r || y + r >= h as i32) {
                            0
                        } else {
                            ((y - r).max(0)..=(y + r).min(h as i32 - 1))
                                .map(|sy| horizontal[(sy as u32 * w + x as u32) as usize])
                                .fold(if grow { 0 } else { 255 }, |a, b| if grow { a.max(b) } else { a.min(b) })
                        };
                        assert_eq!(actual.data()[((y as u32 * w + x as u32) * 4 + 3) as usize], expected, "{w}×{h}, radius {radius}, ({x},{y})");
                    }
                }
            }
        }
    }

    #[test]
    fn broad_blur_tracks_the_full_resolution_reference() {
        let mut exact = Pixmap::new(256, 256).unwrap();
        for y in 80..176 {
            for x in 64..192 {
                let i = (y * 256 + x) * 4;
                exact.data_mut()[i..i + 4].copy_from_slice(&[180, 90, 40, 200]);
            }
        }
        let mut fast = exact.clone();
        blur_exact(&mut exact, 96.0);
        blur(&mut fast, 96.0);
        let differences: Vec<_> = exact.data().iter().zip(fast.data()).map(|(a, b)| a.abs_diff(*b)).collect();
        let mean = differences.iter().map(|v| f64::from(*v)).sum::<f64>() / differences.len() as f64;
        let max = differences.iter().copied().max().unwrap();
        assert!(mean <= 1.5 && max <= 8, "mean {mean}, max {max}");
    }

    #[test]
    fn cropped_blur_matches_full_soft_effect_in_visible_area() {
        for sigma in [20.0, 70.0, 112.0, 220.0] {
            let mut full = Pixmap::new(256, 240).unwrap();
            for y in 40..200 {
                for x in 48..208 {
                    let i = (y * 256 + x) * 4;
                    full.data_mut()[i..i + 4].copy_from_slice(&[120, 60, 30, 180]);
                }
            }
            let source = full.clone();
            blur(&mut full, sigma);
            let cropped = blur_cropped(source, sigma, (40, 30), (144, 160)).unwrap();
            for y in 0..160usize {
                let start = ((y + 30) * 256 + 40) * 4;
                let end = (y * 144) * 4;
                assert_eq!(&cropped.data()[end..end + 144 * 4], &full.data()[start..start + 144 * 4], "sigma {sigma}, row {y}");
            }
        }
    }

    #[test]
    fn box_sizes_sane() {
        for s in [0.5, 1.0, 4.0, 50.0] {
            let b = box_sizes(s);
            assert!(b.iter().all(|r| *r < 1000));
        }
    }
}
