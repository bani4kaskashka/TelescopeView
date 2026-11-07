//! The network: a short stack of 3x3 convolutions over brightness, turning each input pixel
//! into the four corrections for its 2x2 output pixels. Hand-written so nothing else ships.

/// Why a model file was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    Magic,
    Shape,
    Length,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ModelError::Magic => "not an enlarge model",
            ModelError::Shape => "unsupported network shape",
            ModelError::Length => "wrong file length",
        })
    }
}

impl std::error::Error for ModelError {}

const OUT_CHANNELS: usize = 4;
const TAPS: usize = 9;
/// Slope of the activation below zero. The same as LEAK in train/train.py.
const LEAK: f32 = 0.1;

struct Layer {
    inputs: usize,
    outputs: usize,
    /// `outputs * inputs * 9` numbers, row by row inside each 3x3.
    kernel: Vec<f32>,
    bias: Vec<f32>,
}

pub struct Model {
    layers: Vec<Layer>,
    channels: usize,
}

impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Model({} layers, {} channels)", self.layers.len(), self.channels)
    }
}

/// Feature maps for one thread.
#[derive(Default)]
pub struct Work {
    a: Vec<f32>,
    b: Vec<f32>,
}

impl Model {
    /// File format, little endian: `ENL1`, layer count, channels, two zero bytes, then for each
    /// layer an f32 scale followed by its kernel and bias as i16 values to multiply by it.
    pub fn from_bytes(bytes: &[u8]) -> Result<Model, ModelError> {
        let header = bytes.get(..8).ok_or(ModelError::Length)?;
        if &header[..4] != b"ENL1" {
            return Err(ModelError::Magic);
        }
        let (count, channels) = (header[4] as usize, header[5] as usize);
        if count < 2 || channels == 0 {
            return Err(ModelError::Shape);
        }
        let mut rest = &bytes[8..];
        let mut layers = Vec::with_capacity(count);
        for i in 0..count {
            let inputs = if i == 0 { 1 } else { channels };
            let outputs = if i + 1 == count { OUT_CHANNELS } else { channels };
            let weights = outputs * inputs * TAPS;
            let need = 4 + (weights + outputs) * 2;
            if rest.len() < need {
                return Err(ModelError::Length);
            }
            let (layer, tail) = rest.split_at(need);
            rest = tail;
            let scale = f32::from_le_bytes([layer[0], layer[1], layer[2], layer[3]]);
            let mut values = layer[4..]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 * scale);
            let kernel: Vec<f32> = values.by_ref().take(weights).collect();
            let bias: Vec<f32> = values.collect();
            layers.push(Layer { inputs, outputs, kernel, bias });
        }
        if !rest.is_empty() {
            return Err(ModelError::Length);
        }
        Ok(Model { layers, channels })
    }

    /// How many pixels away an output pixel can see.
    pub fn radius(&self) -> usize {
        self.layers.len()
    }

    /// Buffers big enough for a square tile of `side` pixels, margins included.
    pub(crate) fn work_for(&self, side: usize) -> Work {
        let size = self.channels.max(OUT_CHANNELS) * side * side;
        Work { a: vec![0.0; size], b: vec![0.0; size] }
    }

    /// Adds the network's corrections to `big`, the plain interpolation of the tile.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_detail(
        &self,
        y: &[f32],
        ew: usize,
        eh: usize,
        margin: usize,
        tw: usize,
        th: usize,
        strength: f32,
        big: &mut [f32],
        work: &mut Work,
    ) {
        if strength == 0.0 {
            return;
        }
        // Only the part the output can see; a larger margin than needed is cropped off here.
        let trim = margin - self.radius();
        let (mut w, mut h) = (ew - 2 * trim, eh - 2 * trim);
        for row in 0..h {
            let from = &y[(row + trim) * ew + trim..][..w];
            for (d, s) in work.a[row * w..][..w].iter_mut().zip(from) {
                *d = *s - 0.5;
            }
        }
        let last = self.layers.len() - 1;
        for (i, layer) in self.layers.iter().enumerate() {
            convolve(layer, &work.a, w, h, &mut work.b, i != last);
            std::mem::swap(&mut work.a, &mut work.b);
            w -= 2;
            h -= 2;
        }
        debug_assert_eq!((w, h), (tw, th));

        // Each input pixel's four outputs go to its 2x2 block, as torch's PixelShuffle does.
        let ow = tw * 2;
        let plane = tw * th;
        for sy in 0..th {
            for sx in 0..tw {
                let i = sy * tw + sx;
                for c in 0..OUT_CHANNELS {
                    let (dy, dx) = (c / 2, c % 2);
                    big[(sy * 2 + dy) * ow + sx * 2 + dx] += strength * work.a[c * plane + i];
                }
            }
        }
    }
}

/// Runs one layer with the best instructions this CPU has. The binary still starts on a CPU
/// without them, which simply takes the plain path.
fn convolve(layer: &Layer, input: &[f32], w: usize, h: usize, out: &mut [f32], activate: bool) {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma") {
            // SAFETY: both features were just detected on this CPU.
            unsafe { convolve_avx2(layer, input, w, h, out, activate) };
            return;
        }
        convolve_body::<false>(layer, input, w, h, out, activate);
    }
    // NEON has a fused multiply-add on every aarch64 CPU.
    #[cfg(target_arch = "aarch64")]
    convolve_body::<true>(layer, input, w, h, out, activate);
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    convolve_body::<false>(layer, input, w, h, out, activate);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
fn convolve_avx2(layer: &Layer, input: &[f32], w: usize, h: usize, out: &mut [f32], activate: bool) {
    convolve_body::<true>(layer, input, w, h, out, activate);
}

#[inline(always)]
fn madd<const FUSED: bool>(a: f32, b: f32, c: f32) -> f32 {
    // Without FMA instructions mul_add falls back to a very slow library call.
    if FUSED { a.mul_add(b, c) } else { a * b + c }
}

/// Valid convolution: the output is 2 pixels smaller each way. Channels are stored one after
/// another, so each input/output pair only touches two small blocks of memory.
#[inline(always)]
fn convolve_body<const FUSED: bool>(
    layer: &Layer,
    input: &[f32],
    w: usize,
    h: usize,
    out: &mut [f32],
    activate: bool,
) {
    let (ow, oh) = (w - 2, h - 2);
    for o in 0..layer.outputs {
        let dst = &mut out[o * ow * oh..][..ow * oh];
        dst.fill(layer.bias[o]);
        for i in 0..layer.inputs {
            let src = &input[i * w * h..][..w * h];
            let taps = &layer.kernel[(o * layer.inputs + i) * TAPS..][..TAPS];
            for (y, line) in dst.chunks_exact_mut(ow).enumerate() {
                for (r, k) in taps.chunks_exact(3).enumerate() {
                    let row = &src[(y + r) * w..][..w];
                    let (k0, k1, k2) = (k[0], k[1], k[2]);
                    let (a, b, c) = (&row[..ow], &row[1..1 + ow], &row[2..2 + ow]);
                    for (d, ((a, b), c)) in line.iter_mut().zip(a.iter().zip(b).zip(c)) {
                        *d = madd::<FUSED>(k0, *a, madd::<FUSED>(k1, *b, madd::<FUSED>(k2, *c, *d)));
                    }
                }
            }
        }
        if activate {
            for v in dst.iter_mut() {
                *v = if *v > 0.0 { *v } else { *v * LEAK };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(layers: u8, channels: u8) -> Vec<u8> {
        let mut v = b"ENL1".to_vec();
        v.extend_from_slice(&[layers, channels, 0, 0]);
        for i in 0..layers as usize {
            let inputs = if i == 0 { 1 } else { channels as usize };
            let outputs = if i + 1 == layers as usize { 4 } else { channels as usize };
            v.extend_from_slice(&0.001f32.to_le_bytes());
            v.extend(std::iter::repeat_n(0u8, (outputs * inputs * 9 + outputs) * 2));
        }
        v
    }

    #[test]
    fn loads_a_well_formed_model() {
        let m = Model::from_bytes(&encode(4, 8)).unwrap();
        assert_eq!(m.radius(), 4);
    }

    #[test]
    fn refuses_broken_files() {
        assert_eq!(Model::from_bytes(b"nope").unwrap_err(), ModelError::Length);
        assert_eq!(Model::from_bytes(b"NOPE0000").unwrap_err(), ModelError::Magic);
        assert_eq!(Model::from_bytes(&encode(1, 8)).unwrap_err(), ModelError::Shape);
        assert_eq!(Model::from_bytes(&encode(4, 0)).unwrap_err(), ModelError::Shape);

        let mut short = encode(4, 8);
        short.pop();
        assert_eq!(Model::from_bytes(&short).unwrap_err(), ModelError::Length);
        let mut long = encode(4, 8);
        long.push(0);
        assert_eq!(Model::from_bytes(&long).unwrap_err(), ModelError::Length);
    }

    #[test]
    fn plain_and_fast_paths_agree() {
        // The path a CPU gets must not change the picture beyond rounding.
        let layer = Layer {
            inputs: 2,
            outputs: 2,
            kernel: (0..36).map(|i| (i as f32 - 17.0) / 40.0).collect(),
            bias: vec![0.1, -0.2],
        };
        let (w, h) = (20, 11);
        let input: Vec<f32> = (0..2 * w * h).map(|i| ((i * 37) % 101) as f32 / 101.0).collect();
        let mut a = vec![0.0; 2 * (w - 2) * (h - 2)];
        let mut b = a.clone();
        convolve_body::<false>(&layer, &input, w, h, &mut a, true);
        convolve(&layer, &input, w, h, &mut b, true);
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 1e-4);
        }
    }

    /// `tests/golden.bin` comes from the training script: the same network run by PyTorch. If
    /// this passes, the Rust runtime computes what was trained.
    #[test]
    fn matches_pytorch() {
        let g = include_bytes!("golden.bin");
        let read_u32 = |at: usize| u32::from_le_bytes(g[at..at + 4].try_into().unwrap()) as usize;
        let (model_len, side) = (read_u32(0), read_u32(4));
        let model = Model::from_bytes(&g[8..8 + model_len]).unwrap();
        let floats: Vec<f32> = g[8 + model_len..]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let margin = model.radius().max(crate::enlarge::plane::MIN_MARGIN);
        let extended = side + 2 * margin;
        let (input, expected) = floats.split_at(extended * extended);
        assert_eq!(expected.len(), 4 * side * side);

        let mut big = vec![0.0; 4 * side * side];
        let mut temp = vec![0.0; 2 * side * (side + 4)];
        crate::enlarge::plane::interpolate(input, extended, margin, side, side, &mut big, &mut temp);
        let mut work = model.work_for(extended);
        model.add_detail(input, extended, extended, margin, side, side, 1.0, &mut big, &mut work);

        for (i, (got, want)) in big.iter().zip(expected).enumerate() {
            assert!((got - want).abs() < 2e-4, "pixel {i}: rust {got}, torch {want}");
        }
    }
}
