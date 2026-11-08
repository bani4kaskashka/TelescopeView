//! Windows' own OCR (Windows.Media.Ocr): in every Windows 10 and 11, reads the languages
//! installed in Settings, needs nothing shipped with the app.

use image::RgbaImage;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

/// It takes up to 10000 px, but in a slice that tall it misread words a 4096 one got right.
pub fn max_side() -> u32 {
    OcrEngine::MaxImageDimension().unwrap_or(4096).min(4096)
}

pub fn read(img: &RgbaImage) -> Result<super::Lines, String> {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    // SAFETY: plain COM start-up for this background thread; a second call is harmless.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|_| "Windows has no text recognition for your languages (Settings > Time & language)".to_owned())?;
    let run = || -> windows::core::Result<super::Lines> {
        let mut bgra = img.as_raw().clone();
        for p in bgra.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        let (w, h) = (img.width() as i32, img.height() as i32);
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &writer.DetachBuffer()?,
            BitmapPixelFormat::Bgra8,
            w,
            h,
            BitmapAlphaMode::Ignore,
        )?;
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        let mut lines = Vec::new();
        for line in result.Lines()? {
            let mut words = Vec::new();
            for word in line.Words()? {
                let r = word.BoundingRect()?;
                words.push((word.Text()?.to_string(), [r.X, r.Y, r.X + r.Width, r.Y + r.Height]));
            }
            lines.push(words);
        }
        Ok(lines)
    };
    run().map_err(|e| e.message().to_string())
}
