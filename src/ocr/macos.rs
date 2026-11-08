//! Apple's Vision (the engine behind Live Text): in macOS 10.15 and later, nothing shipped
//! with the app.

use image::{ImageEncoder, RgbaImage};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, NSObjectProtocol};
use objc2::{AnyThread, msg_send, sel};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSRange};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText, VNRectangleObservation, VNRequest,
    VNRequestTextRecognitionLevel,
};

/// Vision shrinks big pictures itself; slicing long ones keeps small text readable.
pub fn max_side() -> u32 {
    4096
}

pub fn read(img: &RgbaImage) -> Result<super::Lines, String> {
    if AnyClass::get(c"VNRecognizeTextRequest").is_none() {
        return Err("reading text needs macOS 10.15 or newer".to_owned());
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    let (w, h) = (img.width() as f64, img.height() as f64);
    autoreleasepool(|_| {
        let data = NSData::with_bytes(&png);
        let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &NSDictionary::new());
        // The class exists: checked above.
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        if request.respondsToSelector(sel!(setAutomaticallyDetectsLanguage:)) {
            // macOS 13 and later; English only before that.
            request.setAutomaticallyDetectsLanguage(true);
        }
        let as_request: &VNRequest = &request;
        handler.performRequests_error(&NSArray::from_slice(&[as_request])).map_err(|e| e.localizedDescription().to_string())?;
        let mut lines = Vec::new();
        for found in request.results().iter().flat_map(|r| r.iter()) {
            let Some(best) = found.topCandidates(1).firstObject() else { continue };
            let text = best.string().to_string();
            // SAFETY: a plain getter.
            let line_box = unsafe { found.boundingBox() };
            let line_box = [line_box.origin.x, line_box.origin.y, line_box.size.width, line_box.size.height];
            let mut words = Vec::new();
            let mut utf16 = 0;
            let mut chars = text.char_indices().peekable();
            let total = text.encode_utf16().count();
            while let Some(&(at, c)) = chars.peek() {
                if c.is_whitespace() {
                    utf16 += c.len_utf16();
                    chars.next();
                    continue;
                }
                let start = utf16;
                let mut end_byte = at;
                while let Some(&(i, c)) = chars.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    utf16 += c.len_utf16();
                    end_byte = i + c.len_utf8();
                    chars.next();
                }
                let word = &text[at..end_byte];
                // Vision boxes are 0..1 with the origin at the bottom left.
                let [x, y, bw, bh] = word_box(&best, start, utf16 - start).unwrap_or_else(|| {
                    // No box of its own: its share of the line's width.
                    let n = total.max(1) as f64;
                    [line_box[0] + line_box[2] * start as f64 / n, line_box[1], line_box[2] * (utf16 - start) as f64 / n, line_box[3]]
                });
                let (l, t) = (x * w, (1.0 - y - bh) * h);
                words.push((word.to_owned(), [l as f32, t as f32, (l + bw * w) as f32, (t + bh * h) as f32]));
            }
            if !words.is_empty() {
                lines.push(words);
            }
        }
        // Vision doesn't promise an order: top to bottom, then left to right.
        lines.sort_by(|a: &Vec<(String, [f32; 4])>, b| (a[0].1[1], a[0].1[0]).partial_cmp(&(b[0].1[1], b[0].1[0])).unwrap_or(std::cmp::Ordering::Equal));
        Ok(lines)
    })
}

/// The box of `len` UTF-16 units from `start` in the recognised text, as `[x, y, w, h]`.
fn word_box(text: &VNRecognizedText, start: usize, len: usize) -> Option<[f64; 4]> {
    // `boundingBoxForRange:error:` isn't in objc2-vision's generated list, so it's sent by hand.
    // SAFETY: the selector exists on VNRecognizedText since macOS 10.15 and takes an NSRange.
    let found: Result<Retained<VNRectangleObservation>, Retained<NSError>> =
        unsafe { msg_send![text, boundingBoxForRange: NSRange::new(start, len), error: _] };
    let b = unsafe { found.ok()?.boundingBox() };
    Some([b.origin.x, b.origin.y, b.size.width, b.size.height])
}
