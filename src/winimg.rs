//! What Windows itself can decode: WIC codecs (HEIC/HEIF, AVIF, JPEG XR, camera RAW, plus
//! anything else installed from the Store) and PDF pages through `Windows.Data.Pdf`. Used for
//! files the built-in Rust decoders don't handle.

use std::path::Path;

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Storage::StorageFile;
    use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;
    use windows::core::HSTRING;

    thread_local! {
        static COM: bool = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };
        /// The PDF a decoder thread opened last: turning pages shouldn't re-parse the file.
        static PDF: RefCell<Option<(PathBuf, PdfDocument)>> = const { RefCell::new(None) };
    }

    fn com() {
        COM.with(|_| {});
    }

    fn err(e: windows::core::Error) -> String {
        e.message().to_string()
    }

    /// RGBA pixels, their size, and the image's original size.
    pub type Decoded = (Vec<u8>, [u32; 2], [u32; 2]);

    /// Decodes with WIC to straight-alpha RGBA, scaled down to fit `max_side` if needed.
    pub fn decode(bytes: &[u8], max_side: u32) -> Result<Decoded, String> {
        com();
        // SAFETY: plain COM calls on interfaces created right here; `bytes` outlives the stream.
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(err)?;
            let stream = factory.CreateStream().map_err(err)?;
            stream.InitializeFromMemory(bytes).map_err(err)?;
            let decoder = factory
                .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
                .map_err(|_| "no decoder for this format (a Windows codec may be missing)".to_owned())?;
            let frame = decoder.GetFrame(0).map_err(err)?;
            let (mut w, mut h) = (0, 0);
            frame.GetSize(&mut w, &mut h).map_err(err)?;
            let original = [w, h];
            let source: IWICBitmapSource = if w > max_side || h > max_side {
                let s = max_side as f64 / w.max(h) as f64;
                let (nw, nh) = (((w as f64 * s) as u32).max(1), ((h as f64 * s) as u32).max(1));
                let scaler = factory.CreateBitmapScaler().map_err(err)?;
                scaler.Initialize(&frame, nw, nh, WICBitmapInterpolationModeFant).map_err(err)?;
                (w, h) = (nw, nh);
                windows::core::Interface::cast(&scaler).map_err(err)?
            } else {
                windows::core::Interface::cast(&frame).map_err(err)?
            };
            let converter = factory.CreateFormatConverter().map_err(err)?;
            converter
                .Initialize(
                    &source,
                    &GUID_WICPixelFormat32bppRGBA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(err)?;
            let mut buf = vec![0u8; w as usize * h as usize * 4];
            converter.CopyPixels(std::ptr::null(), w * 4, &mut buf).map_err(err)?;
            Ok((buf, [w, h], original))
        }
    }

    fn open_pdf(path: &Path) -> windows::core::Result<PdfDocument> {
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?.join()?;
        PdfDocument::LoadFromFileAsync(&file)?.join()
    }

    fn with_pdf<T>(path: &Path, f: impl FnOnce(&PdfDocument) -> windows::core::Result<T>) -> Result<T, String> {
        com();
        PDF.with_borrow_mut(|cached| {
            if cached.as_ref().is_none_or(|(p, _)| p != path) {
                *cached = Some((path.to_path_buf(), open_pdf(path).map_err(err)?));
            }
            f(&cached.as_ref().expect("just set").1).map_err(err)
        })
    }

    pub fn pdf_page_count(path: &Path) -> Result<u32, String> {
        // Its own thread: WinRT's blocking waits are not welcome on the UI thread (an STA).
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            com();
            open_pdf(&path).and_then(|d| d.PageCount()).map_err(err)
        })
        .join()
        .unwrap_or_else(|_| Err("PDF reader crashed".to_owned()))
    }

    /// Renders one page (numbered from 1) to an encoded BMP, `long_side` pixels on its long side.
    pub fn pdf_render(path: &Path, page: u32, long_side: u32) -> Result<Vec<u8>, String> {
        with_pdf(path, |doc| {
            let page = doc.GetPage(page.saturating_sub(1))?;
            let size = page.Size()?;
            let scale = long_side as f32 / size.Width.max(size.Height).max(1.0);
            let opts = PdfPageRenderOptions::new()?;
            opts.SetDestinationWidth((size.Width * scale).round().max(1.0) as u32)?;
            opts.SetDestinationHeight((size.Height * scale).round().max(1.0) as u32)?;
            // BMP: nearly free to encode and decode, unlike the default PNG.
            opts.SetBitmapEncoderId(windows::Graphics::Imaging::BitmapEncoder::BmpEncoderId()?)?;
            let stream = InMemoryRandomAccessStream::new()?;
            page.RenderWithOptionsToStreamAsync(&stream, &opts)?.join()?;
            let len = stream.Size()? as u32;
            let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
            reader.LoadAsync(len)?.join()?;
            let mut buf = vec![0u8; len as usize];
            reader.ReadBytes(&mut buf)?;
            Ok(buf)
        })
    }
}

#[cfg(windows)]
pub use imp::{decode, pdf_render};

pub fn pdf_page_count(path: &Path) -> std::io::Result<u32> {
    #[cfg(windows)]
    return imp::pdf_page_count(path).map_err(std::io::Error::other);
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(std::io::Error::other("PDF needs Windows"))
    }
}
