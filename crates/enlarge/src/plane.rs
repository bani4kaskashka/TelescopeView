//! Colour conversion and the math-only parts: interpolation and halo-free sharpening.

/// Catmull-Rom weights for the two new samples between pixels. The training script uses the
/// same numbers, so the network learns exactly what this interpolation misses.
const NEAR: [f32; 4] = [-0.0703125, 0.8671875, 0.2265625, -0.0234375];
const FAR: [f32; 4] = [-0.0234375, 0.2265625, 0.8671875, -0.0703125];

/// Interpolation reads two pixels either side.
pub const MIN_MARGIN: usize = 2;
/// Sharpening blurs first, which needs one more.
pub const FAST_MARGIN: usize = 3;

#[inline]
pub fn to_ycc(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    (y, (b - y) * 0.564_334, (r - y) * 0.713_267)
}

#[inline]
pub fn to_rgb(y: f32, cb: f32, cr: f32) -> (f32, f32, f32) {
    (y + 1.402 * cr, y - 0.344_136 * cb - 0.714_136 * cr, y + 1.772 * cb)
}

/// Doubles the middle `tw` by `th` pixels of a plane with a `margin` around them.
/// `out` is `2*tw` by `2*th`; `temp` holds `2*tw * (th+4)` values.
pub fn interpolate(
    src: &[f32],
    stride: usize,
    margin: usize,
    tw: usize,
    th: usize,
    out: &mut [f32],
    temp: &mut [f32],
) {
    let ow = tw * 2;
    // Across, for every row the vertical pass will read.
    for (r, line) in temp[..ow * (th + 4)].chunks_exact_mut(ow).enumerate() {
        let row = &src[(margin + r - 2) * stride..][..stride];
        for (x, pair) in line.chunks_exact_mut(2).enumerate() {
            let t = &row[margin + x - 2..margin + x + 3];
            pair[0] = FAR[0] * t[0] + FAR[1] * t[1] + FAR[2] * t[2] + FAR[3] * t[3];
            pair[1] = NEAR[0] * t[1] + NEAR[1] * t[2] + NEAR[2] * t[3] + NEAR[3] * t[4];
        }
    }
    // Then down.
    for y in 0..th {
        let rows: [&[f32]; 5] = std::array::from_fn(|i| &temp[(y + i) * ow..][..ow]);
        let (top, bottom) = out[y * 2 * ow..][..2 * ow].split_at_mut(ow);
        for x in 0..ow {
            let (a, b, c, d, e) = (rows[0][x], rows[1][x], rows[2][x], rows[3][x], rows[4][x]);
            top[x] = FAR[0] * a + FAR[1] * b + FAR[2] * c + FAR[3] * d;
            bottom[x] = NEAR[0] * b + NEAR[1] * c + NEAR[2] * d + NEAR[3] * e;
        }
    }
}

/// The math-only mode: interpolates a sharpened copy, then limits each new pixel to the range
/// of the source pixels around it. That limit is what stops bright halos along edges.
#[allow(clippy::too_many_arguments)]
pub fn sharpen(
    src: &[f32],
    stride: usize,
    margin: usize,
    tw: usize,
    th: usize,
    strength: f32,
    out: &mut [f32],
    temp: &mut [f32],
    boosted: &mut [f32],
) {
    let height = th + 2 * margin;
    boosted[..stride * height].copy_from_slice(&src[..stride * height]);
    // Measured across soft, JPEG and already-sharp pictures: more than this only suits soft ones.
    let amount = 0.32 * strength;
    for y in 1..height - 1 {
        for x in 1..stride - 1 {
            let at = |dx: usize, dy: usize| src[(y + dy - 1) * stride + x + dx - 1];
            let blur = (at(0, 0) + at(2, 0) + at(0, 2) + at(2, 2)
                + 2.0 * (at(1, 0) + at(0, 1) + at(2, 1) + at(1, 2))
                + 4.0 * at(1, 1))
                / 16.0;
            boosted[y * stride + x] = at(1, 1) + amount * (at(1, 1) - blur);
        }
    }
    interpolate(boosted, stride, margin, tw, th, out, temp);

    if strength <= 0.0 {
        return;
    }
    let ow = tw * 2;
    for y in 0..th * 2 {
        // The two source rows and columns closest to this new pixel.
        let sy = margin + y / 2 + (y % 2) - 1;
        for x in 0..ow {
            let sx = margin + x / 2 + (x % 2) - 1;
            let p = sy * stride + sx;
            let (a, b, c, d) = (src[p], src[p + 1], src[p + stride], src[p + stride + 1]);
            let low = a.min(b).min(c).min(d) - 0.01;
            let high = a.max(b).max(c).max(d) + 0.01;
            let v = &mut out[y * ow + x];
            *v = v.clamp(low, high);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_conversion_round_trips() {
        for (r, g, b) in [(0u8, 0u8, 0u8), (255, 255, 255), (255, 0, 0), (0, 255, 0), (0, 0, 255), (12, 200, 99)] {
            let (y, cb, cr) = to_ycc(r, g, b);
            let (r2, g2, b2) = to_rgb(y, cb, cr);
            assert!((r2 * 255.0 - r as f32).abs() < 0.5);
            assert!((g2 * 255.0 - g as f32).abs() < 0.5);
            assert!((b2 * 255.0 - b as f32).abs() < 0.5);
        }
    }

    #[test]
    fn weights_add_up_to_one() {
        assert!((NEAR.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!((FAR.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn interpolating_a_ramp_stays_on_the_ramp() {
        // Catmull-Rom reproduces straight lines, which also proves the sample positions.
        let (tw, th, m) = (6, 4, 2);
        let stride = tw + 2 * m;
        let src: Vec<f32> = (0..(th + 2 * m) * stride).map(|i| (i % stride) as f32).collect();
        let mut out = vec![0.0; tw * 2 * th * 2];
        let mut temp = vec![0.0; 2 * tw * (th + 4)];
        interpolate(&src, stride, m, tw, th, &mut out, &mut temp);
        for x in 0..tw * 2 {
            let expected = m as f32 + (x as f32 - 0.5) / 2.0;
            assert!((out[x] - expected).abs() < 1e-4, "{x}: {} vs {expected}", out[x]);
        }
    }

    #[test]
    fn sharpening_does_not_overshoot_a_hard_edge() {
        let (tw, th, m) = (8, 4, 3);
        let stride = tw + 2 * m;
        let src: Vec<f32> =
            (0..(th + 2 * m) * stride).map(|i| if i % stride < stride / 2 { 0.2 } else { 0.8 }).collect();
        let mut out = vec![0.0; tw * 2 * th * 2];
        let mut temp = vec![0.0; 2 * tw * (th + 4)];
        let mut boosted = vec![0.0; src.len()];
        sharpen(&src, stride, m, tw, th, 1.0, &mut out, &mut temp, &mut boosted);
        assert!(out.iter().all(|v| (0.189..=0.811).contains(v)));
    }
}
