//! A sharp geometry preview corrected by effects measured at a lower resolution.
//!
//! The correction is signed premultiplied RGBA: a low-resolution faithful render minus
//! the same render with expensive layers omitted. Applying that difference to a sharp
//! render preserves geometry while retaining a provisional approximation of effects.

use lizard_render::{Pixmap, View};
use rayon::prelude::*;

pub struct Residual {
    full: Pixmap,
    base: Pixmap,
    view: View,
}

impl Residual {
    pub fn new(full: Pixmap, base: Pixmap, view: View) -> Option<Self> {
        (full.width() == base.width()
            && full.height() == base.height()
            && full.width() == view.width
            && full.height() == view.height
            && view.scale.is_finite()
            && view.scale > 0.0)
            .then_some(Self { full, base, view })
    }

    pub fn apply(&self, sharp: &mut Pixmap, view: &View) {
        if sharp.width() != view.width
            || sharp.height() != view.height
            || view.width == 0
            || view.height == 0
            || !view.scale.is_finite()
            || view.scale <= 0.0
        {
            return;
        }
        let map = |at: u32, origin: f64, source_origin: f64, extent: u32| {
            let world = origin + (f64::from(at) + 0.5) / view.scale;
            let source = (world - source_origin) * self.view.scale - 0.5;
            if !(source.is_finite() && source >= -0.5 && source < f64::from(extent) - 0.5) {
                return None;
            }
            let lo = source.floor().clamp(0.0, f64::from(extent - 1)) as usize;
            let hi = (lo + 1).min(extent as usize - 1);
            Some((lo, hi, (source - lo as f64).clamp(0.0, 1.0) as f32))
        };
        let xs: Vec<_> = (0..view.width).map(|x| map(x, view.origin.0, self.view.origin.0, self.view.width)).collect();
        let ys: Vec<_> = (0..view.height).map(|y| map(y, view.origin.1, self.view.origin.1, self.view.height)).collect();
        let row = self.view.width as usize;
        let (full, base) = (self.full.data(), self.base.data());
        sharp.data_mut().par_chunks_exact_mut(view.width as usize * 4).enumerate().for_each(|(y, output)| {
            let Some((y0, y1, fy)) = ys[y] else { return };
            for (x, mapped_x) in xs.iter().enumerate() {
                let Some((x0, x1, fx)) = *mapped_x else { continue };
                let dst = x * 4;
                let p00 = (y0 * row + x0) * 4;
                let p10 = (y0 * row + x1) * 4;
                let p01 = (y1 * row + x0) * 4;
                let p11 = (y1 * row + x1) * 4;
                for channel in 0..4 {
                    let delta = |at: usize| f32::from(full[at + channel]) - f32::from(base[at + channel]);
                    let top = delta(p00) * (1.0 - fx) + delta(p10) * fx;
                    let bottom = delta(p01) * (1.0 - fx) + delta(p11) * fx;
                    output[dst + channel] = (f32::from(output[dst + channel]) + top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_signed_correction_without_changing_geometry_outside_source() -> Result<(), Box<dyn std::error::Error>> {
        let mut full = Pixmap::new(2, 2).ok_or("full image allocation failed")?;
        let mut base = Pixmap::new(2, 2).ok_or("base image allocation failed")?;
        let mut sharp = Pixmap::new(4, 4).ok_or("sharp image allocation failed")?;
        for pixel in full.data_mut().chunks_exact_mut(4) {
            pixel.copy_from_slice(&[100, 80, 60, 255]);
        }
        for pixel in base.data_mut().chunks_exact_mut(4) {
            pixel.copy_from_slice(&[50, 40, 30, 255]);
        }
        for pixel in sharp.data_mut().chunks_exact_mut(4) {
            pixel.copy_from_slice(&[20, 20, 20, 255]);
        }
        let low = View { origin: (0.0, 0.0), scale: 1.0, width: 2, height: 2 };
        let high = View { origin: (0.0, 0.0), scale: 2.0, width: 4, height: 4 };
        let residual = Residual::new(full, base, low).ok_or("residual dimensions invalid")?;
        residual.apply(&mut sharp, &high);
        assert_eq!(&sharp.data()[0..4], &[70, 60, 50, 255]);
        assert_eq!(&sharp.data()[sharp.data().len() - 4..], &[70, 60, 50, 255]);

        let outside = View { origin: (4.0, 4.0), ..high };
        residual.apply(&mut sharp, &outside);
        assert_eq!(&sharp.data()[0..4], &[70, 60, 50, 255]);
        Ok(())
    }
}
