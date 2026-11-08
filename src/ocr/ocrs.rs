//! ocrs, for Linux: pure Rust, so it builds for any processor, but it reads Latin letters only
//! and needs its two model files, shipped in an `ocr` folder beside the app (or in
//! /usr/share/telescopeview/ocr for the .deb).

use image::RgbaImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use std::path::PathBuf;
use std::sync::OnceLock;

const DETECTION: &str = "text-detection.onnx";
const RECOGNITION: &str = "text-recognition.onnx";

/// The detection model works on a fixed-size input and squeezes bigger pictures into it, so
/// long ones go in slices about this long.
pub fn max_side() -> u32 {
    1600
}

pub fn read(img: &RgbaImage) -> Result<super::Lines, String> {
    static ENGINE: OnceLock<Result<OcrEngine, String>> = OnceLock::new();
    let engine = ENGINE.get_or_init(load).as_ref().map_err(Clone::clone)?;
    let rgb: Vec<u8> = img.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let source = ImageSource::from_bytes(&rgb, img.dimensions()).map_err(text)?;
    let input = engine.prepare_input(source).map_err(text)?;
    let boxes = engine.detect_words(&input).map_err(text)?;
    let line_boxes = engine.find_text_lines(&input, &boxes);
    let mut lines = Vec::new();
    for line in engine.recognize_text(&input, &line_boxes).map_err(text)?.into_iter().flatten() {
        let words: Vec<_> = line
            .words()
            .map(|w| {
                let r = w.bounding_rect();
                (w.to_string(), [r.left() as f32, r.top() as f32, r.right() as f32, r.bottom() as f32])
            })
            .collect();
        lines.push(words);
    }
    Ok(lines)
}

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn load() -> Result<OcrEngine, String> {
    let dir = model_dirs()
        .into_iter()
        .find(|d| d.join(DETECTION).is_file() && d.join(RECOGNITION).is_file())
        .ok_or("the text recognition models aren't installed (an ocr folder next to the app)")?;
    let model = |name: &str| rten::Model::load_file(dir.join(name)).map_err(|e| format!("{name}: {e}"));
    OcrEngine::new(OcrEngineParams {
        detection_model: Some(model(DETECTION)?),
        recognition_model: Some(model(RECOGNITION)?),
        ..Default::default()
    })
    .map_err(|e| e.to_string())
}

fn model_dirs() -> Vec<PathBuf> {
    let exe = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf()));
    let mut dirs = Vec::new();
    if let Some(exe) = exe {
        dirs.push(exe.join("ocr"));
        // /usr/bin/telescopeview -> /usr/share/telescopeview/ocr
        dirs.push(exe.join("../share/telescopeview/ocr"));
    }
    dirs.push(PathBuf::from("/usr/share/telescopeview/ocr"));
    dirs
}
