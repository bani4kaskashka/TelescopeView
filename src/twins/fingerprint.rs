//! What a picture is reduced to for comparing, and how alike two of those are.
//!
//! Two steps, because comparing everything with everything has to stay cheap:
//! - `might_match` throws out almost every pair. It takes a 64-bit perceptual hash (low
//!   frequencies of a DCT) or a coarse 8 x 8 thumbnail, as either can see what the other can't:
//!   on smooth pictures (gradients, blank pages) the hash is mostly noise, and the thumbnail
//!   misses copies with changed brightness;
//! - pairs that survive get a real score: SSIM over a 32 x 32 grey thumbnail, times a colour
//!   check, since the grey part alone can't tell a red shirt from a blue one.

use std::path::Path;

/// A decoded picture, top-left first, 4 bytes per pixel, not premultiplied.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Size of the file's picture when `width` x `height` was scaled down for speed.
    pub full_size: [u32; 2],
}

const SIDE: usize = 32;
const CHROMA_SIDE: usize = 8;
/// SSIM is taken over blocks this wide. Bigger blocks stop noticing a small local change.
const BLOCK: usize = 4;
/// How hard a colour difference pulls the score down.
const COLOUR_WEIGHT: f32 = 3.0;
const COARSE_SIDE: usize = 8;
/// Hash bits (of 63) a pair may differ by and still get a real score. Unrelated pictures differ
/// by about 31, re-encoded or resized copies by under 10.
const MAX_HASH_DISTANCE: u32 = 24;
/// Same for the coarse thumbnail, as a total over its cells: 12 levels each on average.
const MAX_COARSE_DISTANCE: u32 = 12 * (COARSE_SIDE * COARSE_SIDE) as u32;

#[derive(Clone)]
pub struct Fingerprint {
    hash: u64,
    coarse: [u8; COARSE_SIDE * COARSE_SIDE],
    luma: Box<[u8; SIDE * SIDE]>,
    /// Cb and Cr, side by side.
    chroma: Box<[[u8; 2]; CHROMA_SIDE * CHROMA_SIDE]>,
}

impl Fingerprint {
    pub fn of(pic: &Picture) -> Option<Self> {
        let (w, h) = (pic.width as usize, pic.height as usize);
        if w == 0 || h == 0 || pic.rgba.len() < w * h * 4 {
            return None;
        }
        let rgb = shrink(pic, w, h);

        let mut luma = Box::new([0u8; SIDE * SIDE]);
        for (out, c) in luma.iter_mut().zip(&rgb) {
            *out = (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]).round() as u8;
        }

        let mut chroma = Box::new([[0u8; 2]; CHROMA_SIDE * CHROMA_SIDE]);
        let step = SIDE / CHROMA_SIDE;
        let area = (step * step) as f32;
        for cy in 0..CHROMA_SIDE {
            for cx in 0..CHROMA_SIDE {
                let (mut cb, mut cr) = (0.0, 0.0);
                for y in cy * step..(cy + 1) * step {
                    for x in cx * step..(cx + 1) * step {
                        let c = rgb[y * SIDE + x];
                        cb += 128.0 - 0.168736 * c[0] - 0.331264 * c[1] + 0.5 * c[2];
                        cr += 128.0 + 0.5 * c[0] - 0.418688 * c[1] - 0.081312 * c[2];
                    }
                }
                chroma[cy * CHROMA_SIDE + cx] = [(cb / area).round() as u8, (cr / area).round() as u8];
            }
        }

        let mut coarse = [0u8; COARSE_SIDE * COARSE_SIDE];
        let step = SIDE / COARSE_SIDE;
        for cy in 0..COARSE_SIDE {
            for cx in 0..COARSE_SIDE {
                let mut sum = 0u32;
                for y in cy * step..(cy + 1) * step {
                    for x in cx * step..(cx + 1) * step {
                        sum += luma[y * SIDE + x] as u32;
                    }
                }
                coarse[cy * COARSE_SIDE + cx] = (sum / (step * step) as u32) as u8;
            }
        }

        Some(Self { hash: phash(&luma), coarse, luma, chroma })
    }

    /// The cheap check: is this pair worth a real score?
    pub fn might_match(&self, other: &Self) -> bool {
        (self.hash ^ other.hash).count_ones() <= MAX_HASH_DISTANCE
            || self.coarse.iter().zip(&other.coarse).map(|(a, b)| a.abs_diff(*b) as u32).sum::<u32>() <= MAX_COARSE_DISTANCE
    }

    /// 0.0 (nothing alike) to 1.0 (the thumbnails are the same).
    pub fn similarity(&self, other: &Self) -> f32 {
        let structure = ssim(&self.luma, &other.luma).max(0.0);
        let colour = self
            .chroma
            .iter()
            .zip(other.chroma.iter())
            .map(|(a, b)| (a[0].abs_diff(b[0]) as u32 + a[1].abs_diff(b[1]) as u32) as f32)
            .sum::<f32>()
            / (2.0 * 255.0 * self.chroma.len() as f32);
        structure * (1.0 - colour * COLOUR_WEIGHT).max(0.0)
    }
}

/// Averages the whole picture down to `SIDE` x `SIDE` so nothing aliases, whatever its shape.
/// See-through pixels count as mid-grey.
fn shrink(pic: &Picture, w: usize, h: usize) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(SIDE * SIDE);
    for j in 0..SIDE {
        let (y0, y1) = span(j, h);
        for i in 0..SIDE {
            let (x0, x1) = span(i, w);
            let mut sum = [0u64; 3];
            for y in y0..y1 {
                let row = &pic.rgba[(y * w + x0) * 4..(y * w + x1) * 4];
                for p in row.as_chunks::<4>().0 {
                    let a = p[3] as u64;
                    let grey = 128 * (255 - a);
                    sum[0] += p[0] as u64 * a + grey;
                    sum[1] += p[1] as u64 * a + grey;
                    sum[2] += p[2] as u64 * a + grey;
                }
            }
            let n = (255 * (x1 - x0) * (y1 - y0)) as f32;
            out.push([sum[0] as f32 / n, sum[1] as f32 / n, sum[2] as f32 / n]);
        }
    }
    out
}

/// The source pixels behind output cell `i`. Never empty, so tiny pictures work too.
fn span(i: usize, size: usize) -> (usize, usize) {
    let start = i * size / SIDE;
    (start, ((i + 1) * size / SIDE).max(start + 1))
}

fn phash(luma: &[u8; SIDE * SIDE]) -> u64 {
    const KEEP: usize = 8;
    // cos((2x + 1) * u * pi / 2N), the DCT's basis.
    let mut basis = [[0f32; SIDE]; KEEP];
    for (u, row) in basis.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            *v = ((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI / (2 * SIDE) as f32).cos();
        }
    }
    let mut rows = [[0f32; KEEP]; SIDE];
    for (y, row) in rows.iter_mut().enumerate() {
        for (u, out) in row.iter_mut().enumerate() {
            *out = (0..SIDE).map(|x| luma[y * SIDE + x] as f32 * basis[u][x]).sum();
        }
    }
    let mut coefficients = [0f32; KEEP * KEEP];
    for v in 0..KEEP {
        for u in 0..KEEP {
            coefficients[v * KEEP + u] = (0..SIDE).map(|y| rows[y][u] * basis[v][y]).sum();
        }
    }
    // The first one is just the average brightness, so it isn't part of the picture's shape.
    let mut sorted = coefficients[1..].to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    coefficients.iter().skip(1).enumerate().fold(0, |hash, (bit, &c)| if c > median { hash | 1 << bit } else { hash })
}

/// Mean SSIM over small blocks, so a change in one corner still shows up.
fn ssim(a: &[u8; SIDE * SIDE], b: &[u8; SIDE * SIDE]) -> f32 {
    const C1: f32 = 6.5025; // (0.01 * 255)^2, the formula's usual constants.
    const C2: f32 = 58.5225; // (0.03 * 255)^2
    let n = (BLOCK * BLOCK) as f32;
    let mut total = 0.0;
    for by in (0..SIDE).step_by(BLOCK) {
        for bx in (0..SIDE).step_by(BLOCK) {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for y in by..by + BLOCK {
                for x in bx..bx + BLOCK {
                    let (va, vb) = (a[y * SIDE + x] as f32, b[y * SIDE + x] as f32);
                    sa += va;
                    sb += vb;
                    saa += va * va;
                    sbb += vb * vb;
                    sab += va * vb;
                }
            }
            let (ma, mb) = (sa / n, sb / n);
            let (va, vb, cov) = (saa / n - ma * ma, sbb / n - mb * mb, sab / n - ma * mb);
            total += ((2.0 * ma * mb + C1) * (2.0 * cov + C2)) / ((ma * ma + mb * mb + C1) * (va + vb + C2));
        }
    }
    total / ((SIDE / BLOCK) * (SIDE / BLOCK)) as f32
}

/// Decodes with the `image` crate only. The real app passes its own decoder, which also knows
/// HEIC, RAW, PSD and the rest.
pub fn decode(path: &Path, max_side: u32) -> Result<Picture, String> {
    use image::{DynamicImage, ImageDecoder, ImageReader};
    let reader = ImageReader::open(path).map_err(|e| e.to_string())?;
    let mut decoder =
        reader.with_guessed_format().map_err(|e| e.to_string())?.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    img.apply_orientation(orientation);
    let full_size = [img.width(), img.height()];
    if img.width().max(img.height()) > max_side {
        img = img.thumbnail(max_side, max_side);
    }
    let rgba = img.into_rgba8();
    Ok(Picture { width: rgba.width(), height: rgba.height(), full_size, rgba: rgba.into_raw() })
}

/// The file types `decode` can read.
pub fn is_image(path: &Path) -> bool {
    const EXTENSIONS: &[&str] = &[
        "png", "apng", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "bmp", "dib", "tif", "tiff", "ico", "tga", "qoi",
        "pnm", "pbm", "pgm", "ppm", "pam", "hdr", "exr", "dds", "ff",
    ];
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth pattern with a seed, so that different seeds give different pictures.
    fn picture(w: u32, h: u32, seed: f32) -> Picture {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let wave = |a: f32, b: f32| ((u * a + seed).sin() + (v * b + seed * 1.7).cos()) * 60.0 + 128.0;
                rgba.extend([wave(9.0, 7.0) as u8, wave(5.0, 11.0) as u8, wave(13.0, 3.0) as u8, 255]);
            }
        }
        Picture { width: w, height: h, rgba, full_size: [w, h] }
    }

    fn fp(p: &Picture) -> Fingerprint {
        Fingerprint::of(p).unwrap()
    }

    #[test]
    fn same_picture_scores_one() {
        let p = fp(&picture(300, 200, 1.0));
        assert!(p.might_match(&p));
        assert!((p.similarity(&p) - 1.0).abs() < 1e-4);
    }

    /// These are smooth, the kind of picture whose hash is unreliable.
    #[test]
    fn size_barely_matters() {
        let (big, small) = (fp(&picture(1200, 800, 1.0)), fp(&picture(300, 200, 1.0)));
        assert!(big.might_match(&small));
        assert!(big.similarity(&small) > 0.97, "{}", big.similarity(&small));
    }

    #[test]
    fn unrelated_pictures_rarely_get_a_real_score() {
        let prints: Vec<Fingerprint> = (0..12).map(|n| fp(&picture(120, 80, n as f32 * 2.3 + 0.5))).collect();
        let mut candidates = 0;
        for (i, a) in prints.iter().enumerate() {
            candidates += prints[i + 1..].iter().filter(|b| a.might_match(b)).count();
        }
        assert!(candidates < 30, "{candidates} of 66");
    }

    #[test]
    fn different_pictures_score_low() {
        let (a, b) = (fp(&picture(300, 200, 1.0)), fp(&picture(300, 200, 4.0)));
        assert!(a.similarity(&b) < 0.5, "{}", a.similarity(&b));
    }

    #[test]
    fn colour_change_lowers_the_score() {
        let a = picture(300, 200, 1.0);
        let mut swapped = picture(300, 200, 1.0);
        for p in swapped.rgba.as_chunks_mut::<4>().0 {
            let (r, g, b) = (p[0] as u32, p[1] as u32, p[2] as u32);
            let grey = ((r + g + b) / 3) as u8;
            p[..3].copy_from_slice(&[grey, grey, grey]);
        }
        let (a, grey) = (fp(&a), fp(&swapped));
        assert!(a.similarity(&grey) < 0.99);
    }

    #[test]
    fn tiny_and_see_through_pictures_work() {
        let one = Picture { width: 1, height: 1, rgba: vec![10, 20, 30, 255], full_size: [1, 1] };
        let clear = Picture { width: 2, height: 1, rgba: vec![0; 8], full_size: [2, 1] };
        assert!(fp(&one).similarity(&fp(&one)) > 0.99);
        assert!(fp(&clear).might_match(&fp(&clear)));
        assert!(Fingerprint::of(&Picture { width: 5, height: 5, rgba: vec![], full_size: [5, 5] }).is_none());
    }
}
