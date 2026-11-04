//! Things to do with the file itself, Windows-style: delete to the Recycle Bin, save a
//! rotation, rename, copy / move, open with, edit, print, set as wallpaper, properties.
//! Pages inside archives and PDFs are read-only; those actions say so instead.

use crate::app::{App, Start};
use crate::files;
use eframe::egui;
use std::path::{Path, PathBuf};

impl App {
    /// The current page if it's a real file on disk (not a page inside an archive).
    pub fn current_file(&self) -> Option<PathBuf> {
        self.current().filter(|p| p.is_file()).cloned()
    }

    fn need_file(&mut self) -> Option<PathBuf> {
        let f = self.current_file();
        if f.is_none() && self.current().is_some() {
            self.toast("Pages inside an archive or PDF can't be changed");
        }
        f
    }

    /// Drops a page from the list after it was deleted or moved away.
    fn forget_current(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current().cloned() else {
            return;
        };
        self.cache.remove(&path);
        self.rotation.remove(&path);
        self.files.remove(self.index);
        if self.files.is_empty() {
            self.shown = None;
            self.dir = None;
            self.index = 0;
        } else {
            self.index = self.index.min(self.files.len() - 1);
        }
        // History points at a page that still exists.
        self.remember_position();
        self.refresh(ctx);
    }

    /// To the Recycle Bin. On drives without one (USB sticks, FAT32) Windows itself asks
    /// before deleting for good.
    pub fn delete_current(&mut self, ctx: &egui::Context) {
        let Some(path) = self.need_file() else { return };
        if self.settings.confirm_delete {
            self.confirm_delete = Some(path);
        } else {
            self.delete_now(ctx, path);
        }
    }

    pub fn delete_now(&mut self, ctx: &egui::Context, path: std::path::PathBuf) {
        match recycle(&path) {
            Ok(()) if !path.exists() => {
                self.forget_current(ctx);
                self.toast(format!("Deleted {}", files::file_name(&path)));
            }
            Ok(()) => {} // Cancelled in Windows' own dialog.
            Err(e) => self.toast(format!("Couldn't delete: {e}")),
        }
    }

    /// Writes the on-screen rotation into the file. JPEGs only get their orientation flag
    /// changed (no re-compression, nothing lost); lossless formats are saved again.
    pub fn save_rotation(&mut self, ctx: &egui::Context) {
        let Some(path) = self.need_file() else { return };
        let turns = self.current_rotation();
        if turns == 0 {
            let r = self.keymap.text(crate::keys::Action::RotateRight);
            self.toast(format!("Rotate ({r}) or flip first, then save"));
            return;
        }
        match rotate_file(&path, turns) {
            Ok(()) => {
                // Decode again from the changed file. The old picture stays up, still turned
                // on screen, until the new one lands (then the on-screen turn goes).
                self.unrotate.push(path.clone());
                self.cache.remove(&path);
                self.refresh(ctx);
                self.toast("Saved to the file");
            }
            Err(e) => self.toast(format!("Couldn't save it: {e}")),
        }
    }

    pub fn start_rename(&mut self) {
        if let Some(path) = self.need_file() {
            self.renaming = Some(files::file_name(&path));
        }
    }

    pub fn finish_rename(&mut self, ctx: &egui::Context, new_name: &str) {
        self.renaming = None;
        let Some(path) = self.current_file() else {
            return;
        };
        let new_name = new_name.trim();
        if new_name.is_empty() || new_name == files::file_name(&path) {
            return;
        }
        if new_name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
            self.toast("A file name can't contain \\ / : * ? \" < > |");
            return;
        }
        let target = path.with_file_name(new_name);
        // Only the case changes ("a.jpg" to "A.jpg"): Windows says it exists, but it's the same file.
        let same_file = files::file_name(&path).eq_ignore_ascii_case(new_name);
        if target.exists() && !same_file {
            self.toast(format!("{new_name} already exists"));
            return;
        }
        match std::fs::rename(&path, &target) {
            Ok(()) => {
                let dir = self.dir.clone();
                if let Some(dir) = dir {
                    self.open_folder(ctx, dir, Start::File(target));
                }
            }
            Err(e) => self.toast(format!("Couldn't rename: {e}")),
        }
    }

    pub fn copy_or_move_to(&mut self, ctx: &egui::Context, moving: bool) {
        let Some(path) = (if moving { self.need_file() } else { self.current().cloned() }) else {
            return;
        };
        let Some(folder) = rfd::FileDialog::new().set_title(if moving { "Move to" } else { "Copy to" }).pick_folder()
        else {
            return;
        };
        let name = files::file_name(&path);
        let target = folder.join(&name);
        if target.exists() {
            self.toast(format!("{name} is already in {}", files::file_name(&folder)));
            return;
        }
        let result = if moving {
            // Across drives rename fails; then it's a copy, and the original goes only once
            // the copy is complete.
            std::fs::rename(&path, &target)
                .or_else(|_| std::fs::copy(&path, &target).and_then(|_| std::fs::remove_file(&path)))
        } else {
            files::read_page(&path).and_then(|bytes| std::fs::write(&target, bytes))
        };
        match result {
            Ok(()) if moving => {
                self.forget_current(ctx);
                self.toast(format!("Moved to {}", folder.display()));
            }
            Ok(()) => self.toast(format!("Copied to {}", folder.display())),
            Err(e) => self.toast(format!("Couldn't {}: {e}", if moving { "move" } else { "copy" })),
        }
    }

    pub fn shell_verb(&mut self, verb: &str) {
        let Some(path) = self.need_file() else { return };
        if let Err(e) = shell_execute(verb, &path) {
            self.toast(format!("Windows couldn't {verb} it: {e}"));
        }
    }

    /// Windows' "Open with" app picker.
    pub fn open_with(&mut self) {
        let Some(path) = self.need_file() else { return };
        let _ = std::process::Command::new("rundll32.exe").arg("shell32.dll,OpenAs_RunDLL").arg(&path).spawn();
    }

    /// A copy goes next to the settings, so the wallpaper survives unplugging the drive.
    pub fn set_wallpaper(&mut self, ctx: &egui::Context) {
        let Some(page) = self.current().cloned() else {
            return;
        };
        self.in_background(ctx, move || match as_png(&page, "wallpaper.png").and_then(|f| set_wallpaper_file(&f)) {
            Ok(()) => "Set as desktop background".to_owned(),
            Err(e) => format!("Couldn't set the background: {e}"),
        });
    }

    /// Windows only: the picture behind the sign-in screen.
    pub fn set_lock_screen(&mut self, ctx: &egui::Context) {
        let Some(page) = self.current().cloned() else {
            return;
        };
        self.in_background(ctx, move || {
            match as_png(&page, "lock screen.png").and_then(|f| set_lock_screen_file(&f)) {
                Ok(()) => "Set as lock screen".to_owned(),
                Err(e) => format!("Couldn't set the lock screen: {e}"),
            }
        });
    }
}

/// Saves the page as a PNG in the settings folder (pages in archives and odd formats too),
/// for Windows / the desktop to read.
fn as_png(page: &Path, name: &str) -> Result<std::path::PathBuf, String> {
    let img = crate::loader::decode(page, 16384)?;
    let tile = &img.frames[0].tiles[0].pixels;
    let rgba: Vec<u8> = tile.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    let buf = image::RgbaImage::from_raw(tile.size[0] as u32, tile.size[1] as u32, rgba).ok_or("bad image")?;
    let dir = eframe::storage_dir(crate::app::APP_NAME).ok_or("no settings folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(name);
    buf.save(&file).map_err(|e| e.to_string())?;
    Ok(file)
}

#[cfg(windows)]
fn set_lock_screen_file(file: &Path) -> Result<(), String> {
    use windows::Storage::StorageFile;
    use windows::System::UserProfile::LockScreen;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    // SAFETY: plain COM start-up for this background thread; a second call is harmless.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let run = || -> windows::core::Result<()> {
        let file = StorageFile::GetFileFromPathAsync(&windows::core::HSTRING::from(file.as_os_str()))?.join()?;
        LockScreen::SetImageFileAsync(&file)?.join()
    };
    run().map_err(|e| e.message().to_string())
}

#[cfg(not(windows))]
fn set_lock_screen_file(_: &Path) -> Result<(), String> {
    Err("only on Windows".into())
}

#[cfg(windows)]
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn recycle(path: &Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW, SHFileOperationW};
    // pFrom is a list of paths ending in an extra NUL.
    let mut from = wide(path.as_os_str());
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_WANTNUKEWARNING.0) as u16,
        ..Default::default()
    };
    // SAFETY: `from` is a double-NUL-terminated wide string that outlives the call.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code == 0 || op.fAnyOperationsAborted.as_bool() { Ok(()) } else { Err(format!("error {code:#x}")) }
}

#[cfg(windows)]
fn shell_execute(verb: &str, path: &Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::{SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb_w, file_w) = (wide(verb.as_ref()), wide(path.as_os_str()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        // Needed for "properties", harmless for the rest.
        fMask: SEE_MASK_INVOKEIDLIST,
        lpVerb: windows::core::PCWSTR(verb_w.as_ptr()),
        lpFile: windows::core::PCWSTR(file_w.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call; the struct is fully initialised.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| e.message().to_string())
}

#[cfg(windows)]
fn set_wallpaper_file(file: &Path) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoW,
    };
    let mut w = wide(file.as_os_str());
    // SAFETY: a NUL-terminated wide path that outlives the call.
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(w.as_mut_ptr().cast()),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
    }
    .map_err(|e| e.message().to_string())
}

/// To the Trash (macOS) or the desktop's trash folder (Linux).
#[cfg(not(windows))]
fn recycle(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())
}
#[cfg(not(windows))]
fn shell_execute(_: &str, _: &Path) -> Result<(), String> {
    Err("only on Windows".into())
}
#[cfg(target_os = "macos")]
fn set_wallpaper_file(file: &Path) -> Result<(), String> {
    let script = format!(
        "tell application \"System Events\" to tell every desktop to set picture to {:?}",
        file.display().to_string()
    );
    run_ok(std::process::Command::new("osascript").arg("-e").arg(script))
}

/// GNOME and its relatives (Ubuntu, Pop, Cinnamon read the same keys), light and dark.
#[cfg(all(unix, not(target_os = "macos")))]
fn set_wallpaper_file(file: &Path) -> Result<(), String> {
    let uri = format!("file://{}", file.display());
    for key in ["picture-uri", "picture-uri-dark"] {
        let mut cmd = std::process::Command::new("gsettings");
        cmd.args(["set", "org.gnome.desktop.background", key, &uri]);
        // The dark key doesn't exist on older GNOME; the light one is what matters.
        if key == "picture-uri" {
            run_ok(&mut cmd)?;
        } else {
            let _ = cmd.output();
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_ok(cmd: &mut std::process::Command) -> Result<(), String> {
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

/// Turns a file on disk by `turns` (an orientation, see `app::compose`).
fn rotate_file(path: &Path, turns: u8) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let format = image::guess_format(&bytes).map_err(|_| "this format can't be rotated here".to_owned())?;
    let out = match format {
        image::ImageFormat::Jpeg => jpeg_rotate(&bytes, turns)?,
        image::ImageFormat::Png
        | image::ImageFormat::Bmp
        | image::ImageFormat::Tiff
        | image::ImageFormat::Qoi
        | image::ImageFormat::Tga
        | image::ImageFormat::WebP => {
            if is_animated(&bytes, format) {
                return Err("animations can't be rotated".into());
            }
            let mut img = image::load_from_memory_with_format(&bytes, format).map_err(|e| e.to_string())?;
            if turns & 4 != 0 {
                img = img.fliph();
            }
            img = match turns & 3 {
                1 => img.rotate90(),
                2 => img.rotate180(),
                3 => img.rotate270(),
                _ => img,
            };
            let mut out = std::io::Cursor::new(Vec::new());
            // WebP is written lossless: bigger, but the pixels stay exactly as they were.
            img.write_to(&mut out, format).map_err(|e| e.to_string())?;
            out.into_inner()
        }
        _ => return Err("this format can't be rotated here".into()),
    };
    // Written next to it first, so a failure halfway never leaves a broken file.
    let tmp = path.with_extension("telescope-tmp");
    std::fs::write(&tmp, &out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// APNG and animated WebP: rotating would keep only the first frame.
fn is_animated(bytes: &[u8], format: image::ImageFormat) -> bool {
    let cursor = std::io::Cursor::new(bytes);
    match format {
        image::ImageFormat::Png => {
            image::codecs::png::PngDecoder::new(cursor).and_then(|d| d.is_apng()).unwrap_or(false)
        }
        image::ImageFormat::WebP => image::codecs::webp::WebPDecoder::new(cursor).is_ok_and(|d| d.has_animation()),
        _ => false,
    }
}

/// EXIF orientation values for 0, 1, 2, 3 clockwise quarter turns, then the same mirrored
/// left to right first (in the order of `app::compose`'s bits).
const ORIENTATIONS: [u16; 8] = [1, 6, 3, 8, 2, 7, 4, 5];

/// Lossless JPEG rotation: only the EXIF orientation tag changes, the image data is untouched.
fn jpeg_rotate(bytes: &[u8], turns: u8) -> Result<Vec<u8>, String> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return Err("not a JPEG".into());
    }
    let mut out = bytes.to_vec();
    let mut pos = 2;
    let mut after_app0 = 2;
    while pos + 4 <= bytes.len() && bytes[pos] == 0xFF {
        let marker = bytes[pos + 1];
        let len = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
        if marker == 0xDA || len < 2 {
            break; // Image data starts; no more metadata.
        }
        let body = pos + 4..(pos + 2 + len).min(bytes.len());
        if marker == 0xE0 {
            after_app0 = pos + 2 + len;
        }
        if marker == 0xE1 && bytes[body.clone()].starts_with(b"Exif\0\0") {
            let tiff = body.start + 6;
            return match find_orientation(&bytes[tiff..body.end]) {
                Some((offset, big_endian, current)) => {
                    let k = ORIENTATIONS.iter().position(|&o| o == current).unwrap_or(0) as u8;
                    let new = ORIENTATIONS[crate::app::compose(k, turns) as usize];
                    let v = if big_endian { new.to_be_bytes() } else { new.to_le_bytes() };
                    out[tiff + offset..tiff + offset + 2].copy_from_slice(&v);
                    Ok(out)
                }
                None => Err("this JPEG's metadata has no orientation to change".into()),
            };
        }
        pos += 2 + len;
    }
    // No EXIF at all (typical for scans): add a minimal one with just the orientation.
    let value = ORIENTATIONS[turns as usize & 7];
    let mut app1 = vec![0xFF, 0xE1, 0, 34];
    app1.extend_from_slice(b"Exif\0\0MM\0\x2A\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01");
    app1.extend_from_slice(&value.to_be_bytes());
    app1.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    debug_assert_eq!(app1.len(), 36);
    out.splice(after_app0..after_app0, app1);
    Ok(out)
}

/// Finds the orientation tag in IFD0 of a TIFF header: (offset of its value, big endian, value).
fn find_orientation(tiff: &[u8]) -> Option<(usize, bool, u16)> {
    let be = match tiff.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |i: usize| {
        tiff.get(i..i + 2).map(|b| if be { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) })
    };
    let u32_at = |i: usize| {
        tiff.get(i..i + 4).map(|b| {
            let a = [b[0], b[1], b[2], b[3]];
            if be { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) }
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    (0..count).find_map(|i| {
        let entry = ifd + 2 + i * 12;
        (u16_at(entry)? == 0x0112).then(|| Some((entry + 8, be, u16_at(entry + 8)?)))?
    })
}

#[cfg(test)]
pub fn jpeg_rotate_for_tests(bytes: &[u8], turns: u8) -> Vec<u8> {
    jpeg_rotate(bytes, turns).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_fn(4, 2, |x, _| image::Rgb([x as u8 * 60, 0, 0]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
        out.into_inner()
    }

    fn orientation(bytes: &[u8]) -> image::metadata::Orientation {
        use image::ImageDecoder;
        let mut d = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes)).unwrap();
        d.orientation().unwrap()
    }

    #[test]
    fn jpeg_rotation_is_lossless_and_stacks() {
        use image::metadata::Orientation::*;
        let original = jpeg();
        let once = jpeg_rotate(&original, 1).unwrap();
        assert_eq!(orientation(&once), Rotate90);
        assert!(once.ends_with(&original[original.len() - 100..]), "image data untouched");
        let twice = jpeg_rotate(&once, 1).unwrap();
        assert_eq!(twice.len(), once.len(), "second time edits the tag in place");
        assert_eq!(orientation(&twice), Rotate180);
        assert_eq!(orientation(&jpeg_rotate(&twice, 2).unwrap()), NoTransforms);
        let flipped = jpeg_rotate(&original, crate::app::FLIP_H).unwrap();
        assert_eq!(orientation(&flipped), FlipHorizontal);
        assert_eq!(orientation(&jpeg_rotate(&flipped, crate::app::FLIP_V).unwrap()), Rotate180);
        assert_eq!(orientation(&jpeg_rotate(&flipped, 1).unwrap()), Rotate270FlipH, "EXIF 7: transverse");
    }
}
