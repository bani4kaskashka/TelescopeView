//! Becoming the app that opens pictures, videos and comics.
//!
//! The user picks which kinds (and, if they like, which formats) TelescopeView should open;
//! everything else stays with whatever opens it now.
//!
//! - Windows doesn't let an app make itself the default. TelescopeView registers the picked
//!   types under HKEY_CURRENT_USER (no admin rights, nothing for other users) and opens its own
//!   page in Settings > Default apps, where the user confirms each type.
//! - Linux: a .desktop file in ~/.local/share/applications, then `xdg-mime default` for the
//!   picked types (written to ~/.config/mimeapps.list, which GNOME and KDE both read).
//! - macOS: not yet; the app bundle's Info.plist puts it in "Open with".

use crate::files;

/// Camera RAW files, a group of their own: photographers often keep them for their editor.
pub const RAW: &[&str] = &[
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "orf", "rw2", "raf", "pef", "srw", "erf", "kdc",
    "dcr", "mrw", "3fr", "mef", "mos", "x3f", "rwl", "iiq",
];

/// Comic archives (plain zip / rar / 7z / tar, PDFs and EPUBs belong to other apps).
const COMICS: &[&str] = &["cbz", "cbr", "cb7", "cbt"];

/// The kinds of files TelescopeView can be the default for, with their extensions.
pub fn groups() -> [(&'static str, Vec<&'static str>); 4] {
    let pictures = files::EXTENSIONS.iter().filter(|e| !RAW.contains(e)).copied().collect();
    let raw = files::EXTENSIONS.iter().filter(|e| RAW.contains(e)).copied().collect();
    let comics = COMICS.iter().filter(|e| files::CONTAINERS.contains(e)).copied().collect();
    [("Pictures", pictures), ("RAW photos", raw), ("Videos", crate::video::EXTENSIONS.to_vec()), ("Comics", comics)]
}

/// Every extension of every group, except the ones the user keeps for other apps.
fn chosen(off: &[String]) -> Vec<&'static str> {
    groups().into_iter().flat_map(|(_, exts)| exts).filter(|e| !off.iter().any(|o| o == e)).collect()
}

/// A few everyday types, to tell whether it's the default already.
const EVERYDAY: &[&str] = &["jpg", "png", "webp", "gif", "mp4", "mkv", "cbz"];

// --- Windows ---------------------------------------------------------------------------------

#[cfg(windows)]
use crate::app::APP_NAME;

#[cfg(windows)]
const IMAGE_ID: &str = "TelescopeView.Image";
#[cfg(windows)]
const VIDEO_ID: &str = "TelescopeView.Video";
#[cfg(windows)]
const COMIC_ID: &str = "TelescopeView.Comic";
#[cfg(windows)]
const MARK_PROG_ID: &str = "TelescopeView.Bookmark";
#[cfg(windows)]
const CAPABILITIES: &str = r"Software\TelescopeView\Capabilities";

#[cfg(windows)]
fn prog_id(ext: &str) -> &'static str {
    if crate::video::EXTENSIONS.contains(&ext) {
        VIDEO_ID
    } else if files::CONTAINERS.contains(&ext) {
        COMIC_ID
    } else {
        IMAGE_ID
    }
}

/// Registers every type for "Open with", and the ones not in `off` for Default apps (so only
/// those are listed on TelescopeView's page there).
#[cfg(windows)]
fn register(off: &[String]) -> std::io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy();
    let open = format!("\"{exe}\" \"%1\"");
    let icon = format!("\"{exe}\",0");
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    for (id, kind) in [(IMAGE_ID, "Image"), (VIDEO_ID, "Video"), (COMIC_ID, "Comic")] {
        let (prog, _) = hkcu.create_subkey(format!(r"Software\Classes\{id}"))?;
        prog.set_value("", &format!("{kind} ({APP_NAME})"))?;
        prog.create_subkey("DefaultIcon")?.0.set_value("", &icon)?;
        prog.create_subkey(r"shell\open\command")?.0.set_value("", &open)?;
    }

    let (app, _) = hkcu.create_subkey(format!(r"Software\Classes\Applications\{}", exe_name()))?;
    app.set_value("FriendlyAppName", &APP_NAME)?;
    app.create_subkey(r"shell\open\command")?.0.set_value("", &open)?;
    let (types, _) = app.create_subkey("SupportedTypes")?;

    let (caps, _) = hkcu.create_subkey(CAPABILITIES)?;
    caps.set_value("ApplicationName", &APP_NAME)?;
    caps.set_value("ApplicationDescription", &"Fast picture, video and comic viewer")?;
    caps.set_value("ApplicationIcon", &icon)?;
    let (assoc, _) = caps.create_subkey("FileAssociations")?;

    let chosen = chosen(off);
    let all = files::EXTENSIONS.iter().chain(files::CONTAINERS).chain(crate::video::EXTENSIONS);
    for ext in all {
        let id = prog_id(ext);
        let dotted = format!(".{ext}");
        if chosen.contains(ext) {
            assoc.set_value(&dotted, &id)?;
        } else {
            let _ = assoc.delete_value(&dotted);
        }
        types.set_value(&dotted, &"")?;
        let (with, _) = hkcu.create_subkey(format!(r"Software\Classes\{dotted}\OpenWithProgids"))?;
        with.set_value(id, &"")?;
        // Older versions listed everything as an image.
        if id != IMAGE_ID {
            let _ = with.delete_value(IMAGE_ID);
        }
    }
    hkcu.create_subkey(r"Software\RegisteredApplications")?.0.set_value(APP_NAME, &CAPABILITIES)?;
    register_marks()?;
    notify_shell();
    Ok(())
}

/// The types left out of Default apps last time (kept when registering again).
#[cfg(windows)]
fn registered_off() -> Vec<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let Ok(assoc) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(format!(r"{CAPABILITIES}\FileAssociations")) else {
        return Vec::new();
    };
    let has = |e: &str| assoc.get_value::<String, _>(format!(".{e}")).is_ok();
    groups().into_iter().flat_map(|(_, exts)| exts).filter(|e| !has(e)).map(str::to_owned).collect()
}

/// Registers the picked types and opens TelescopeView's page in Default apps, where the
/// user confirms each one. Returns what to tell them.
#[cfg(windows)]
pub fn make_default(off: &[String]) -> Result<String, String> {
    register(off).map_err(|e| format!("Couldn't register: {e}"))?;
    open_default_apps();
    Ok("Pick TelescopeView for each type in that list (Windows asks for every one)".to_owned())
}

/// Bookmark files are ours alone, so they open in TelescopeView on a double-click.
#[cfg(windows)]
fn register_marks() -> std::io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (prog, _) = hkcu.create_subkey(format!(r"Software\Classes\{MARK_PROG_ID}"))?;
    prog.set_value("", &format!("{APP_NAME} bookmark"))?;
    prog.create_subkey("DefaultIcon")?.0.set_value("", &format!("\"{exe}\",0"))?;
    prog.create_subkey(r"shell\open\command")?.0.set_value("", &format!("\"{exe}\" \"%1\""))?;
    let ext = format!(r"Software\Classes\.{}", crate::app::MARK_EXT);
    let (key, _) = hkcu.create_subkey(&ext)?;
    key.set_value("", &MARK_PROG_ID)?;
    key.create_subkey("OpenWithProgids")?.0.set_value(MARK_PROG_ID, &"")?;
    Ok(())
}

#[cfg(windows)]
fn exe_name() -> String {
    std::env::current_exe().ok().map(|p| files::file_name(&p)).unwrap_or_else(|| format!("{APP_NAME}.exe"))
}

/// The exe Windows currently starts for our file type, if we registered before.
#[cfg(windows)]
fn registered_command() -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(format!(r"Software\Classes\{IMAGE_ID}\shell\open\command"))
        .ok()?
        .get_value("")
        .ok()
}

/// Whether the everyday types (the ones the user picked) already open with TelescopeView.
#[cfg(windows)]
pub fn is_default(off: &[String]) -> bool {
    EVERYDAY
        .iter()
        .filter(|e| !off.iter().any(|o| o == *e))
        // Any of its ProgIds (older versions had one for everything), or the exe picked in "Open with".
        .all(|ext| user_choice(ext).is_some_and(|id| id.starts_with("TelescopeView.") || id.starts_with("Applications\\TelescopeView")))
}

/// The app the user picked for a file type. Windows 11 24H2 keeps it in UserChoiceLatest,
/// older ones in UserChoice.
#[cfg(windows)]
fn user_choice(ext: &str) -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let base = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.{ext}");
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    [r"UserChoiceLatest\ProgId", "UserChoice"]
        .iter()
        .find_map(|sub| hkcu.open_subkey(format!(r"{base}\{sub}")).ok()?.get_value("ProgId").ok())
}

/// The command bookmark files open with, if they're registered.
#[cfg(windows)]
fn registered_marks() -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(format!(r"Software\Classes\{MARK_PROG_ID}\shell\open\command"))
        .ok()?
        .get_value("")
        .ok()
}

/// Registers on first start, and again if the registered exe is gone (moved or rebuilt
/// somewhere else). Never takes over from another copy that still exists.
#[cfg(windows)]
pub fn register_if_needed() {
    let stale = match registered_command() {
        None => true,
        Some(cmd) => {
            let registered = cmd.trim_start_matches('"').split('"').next().unwrap_or_default().to_owned();
            !std::path::Path::new(&registered).exists()
        }
    };
    if stale {
        let _ = register(&registered_off());
    } else if registered_marks()
        .is_none_or(|cmd| !cmd.contains(&*std::env::current_exe().unwrap_or_default().to_string_lossy()))
    {
        // Registered before bookmark files existed (or by a copy that's gone since).
        let _ = register_marks();
        notify_shell();
    }
}

/// `--register` / `--unregister`, run by the installer and uninstaller (or by hand). Returns
/// whether the argument was one of them (and the app shouldn't start).
#[cfg(windows)]
pub fn command_line(arg: &std::ffi::OsStr) -> bool {
    match arg.to_str() {
        Some("--register") => {
            let _ = register(&registered_off());
            true
        }
        Some("--unregister") => {
            unregister();
            true
        }
        _ => false,
    }
}

#[cfg(not(windows))]
pub fn command_line(arg: &std::ffi::OsStr) -> bool {
    matches!(arg.to_str(), Some("--register" | "--unregister"))
}

/// Takes back what `register` wrote, if it was this copy of the exe that wrote it (another
/// copy's registration stays).
#[cfg(windows)]
fn unregister() {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    let exe = std::env::current_exe().unwrap_or_default().to_string_lossy().to_lowercase();
    let mine = |cmd: Option<String>| cmd.is_some_and(|c| c.to_lowercase().contains(&exe));
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let classes = r"Software\Classes";
    if mine(registered_command()) {
        for id in [IMAGE_ID, VIDEO_ID, COMIC_ID] {
            let _ = hkcu.delete_subkey_all(format!(r"{classes}\{id}"));
        }
        let _ = hkcu.delete_subkey_all(format!(r"{classes}\Applications\{}", exe_name()));
        let _ = hkcu.delete_subkey_all(CAPABILITIES);
        let _ = hkcu.delete_subkey(r"Software\TelescopeView"); // Only goes if empty.
        if let Ok(apps) = hkcu.open_subkey_with_flags(r"Software\RegisteredApplications", KEY_SET_VALUE) {
            let _ = apps.delete_value(APP_NAME);
        }
        let all = files::EXTENSIONS.iter().chain(files::CONTAINERS).chain(crate::video::EXTENSIONS);
        for ext in all {
            let key = format!(r"{classes}\.{ext}\OpenWithProgids");
            if let Ok(with) = hkcu.open_subkey_with_flags(&key, KEY_SET_VALUE) {
                for id in [IMAGE_ID, VIDEO_ID, COMIC_ID] {
                    let _ = with.delete_value(id);
                }
            }
        }
    }
    if mine(registered_marks()) {
        let _ = hkcu.delete_subkey_all(format!(r"{classes}\{MARK_PROG_ID}"));
        let ext = format!(r"{classes}\.{}", crate::app::MARK_EXT);
        let ours = hkcu
            .open_subkey_with_flags(&ext, KEY_READ)
            .and_then(|k| k.get_value::<String, _>(""))
            .is_ok_and(|id| id == MARK_PROG_ID);
        if ours {
            let _ = hkcu.delete_subkey_all(&ext);
        }
    }
    notify_shell();
}

#[cfg(windows)]
fn notify_shell() {
    use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: the documented "associations changed" broadcast; no pointers are passed.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// Opens Settings at TelescopeView's own page in Default apps.
#[cfg(windows)]
fn open_default_apps() {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::HSTRING;
    // A settings link has to go through the shell; explorer.exe given it as an argument just
    // opens Documents.
    let uri = HSTRING::from(format!("ms-settings:defaultapps?registeredAppUser={APP_NAME}"));
    // SAFETY: plain strings that outlive the call; no window handle needed.
    unsafe { ShellExecuteW(None, &HSTRING::from("open"), &uri, None, None, SW_SHOWNORMAL) };
}

// --- Linux (and other freedesktop systems) ---------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
const DESKTOP: &str = "telescopeview.desktop";

/// The freedesktop MIME type of an extension (shared-mime-info names).
#[cfg(any(all(unix, not(target_os = "macos")), test))]
fn mime(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "apng" => "image/apng",
        "jpg" | "jpeg" | "jpe" | "jfif" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" | "dib" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/vnd.microsoft.icon",
        "cur" => "image/x-win-bitmap",
        "tga" => "image/x-tga",
        "qoi" => "image/qoi",
        "pnm" => "image/x-portable-anymap",
        "pbm" => "image/x-portable-bitmap",
        "pgm" => "image/x-portable-graymap",
        "ppm" => "image/x-portable-pixmap",
        "pam" => "image/x-portable-arbitrarymap",
        "hdr" => "image/vnd.radiance",
        "exr" => "image/x-exr",
        "dds" => "image/x-dds",
        "jxl" => "image/jxl",
        "svg" => "image/svg+xml",
        "svgz" => "image/svg+xml-compressed",
        "psd" => "image/vnd.adobe.photoshop",
        "heic" | "heif" | "hif" => "image/heif",
        "avif" => "image/avif",
        "jxr" | "wdp" | "hdp" => "image/jxr",
        "dng" => "image/x-adobe-dng",
        "cr2" => "image/x-canon-cr2",
        "cr3" => "image/x-canon-cr3",
        "crw" => "image/x-canon-crw",
        "nef" => "image/x-nikon-nef",
        "nrw" => "image/x-nikon-nrw",
        "arw" => "image/x-sony-arw",
        "srf" => "image/x-sony-srf",
        "sr2" => "image/x-sony-sr2",
        "orf" => "image/x-olympus-orf",
        "rw2" => "image/x-panasonic-rw2",
        "raf" => "image/x-fuji-raf",
        "pef" => "image/x-pentax-pef",
        "srw" => "image/x-samsung-srw",
        "erf" => "image/x-epson-erf",
        "kdc" => "image/x-kodak-kdc",
        "dcr" => "image/x-kodak-dcr",
        "mrw" => "image/x-minolta-mrw",
        "x3f" => "image/x-sigma-x3f",
        "mp4" => "video/mp4",
        "m4v" => "video/x-m4v",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "avi" | "divx" => "video/x-msvideo",
        "wmv" => "video/x-ms-wmv",
        "asf" => "video/x-ms-asf",
        "flv" => "video/x-flv",
        "mpg" | "mpeg" | "m2v" | "vob" => "video/mpeg",
        "ts" | "mts" | "m2ts" => "video/mp2t",
        "3gp" => "video/3gpp",
        "3g2" => "video/3gpp2",
        "ogv" => "video/ogg",
        "rm" | "rmvb" => "application/vnd.rn-realmedia",
        "mxf" => "application/mxf",
        "cbz" => "application/vnd.comicbook+zip",
        "cbr" => "application/vnd.comicbook-rar",
        "cb7" => "application/x-cb7",
        "cbt" => "application/x-cbt",
        _ => return None,
    })
}

/// The MIME types of these extensions, each once.
#[cfg(all(unix, not(target_os = "macos")))]
fn mimes<'a>(exts: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for m in exts.into_iter().filter_map(mime) {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    out
}

#[cfg(all(unix, not(target_os = "macos")))]
fn applications_dir() -> Option<std::path::PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".local/share")))?;
    Some(data.join("applications"))
}

/// Writes TelescopeView's .desktop file (listing every type, for "Open with") and makes it the
/// default for the picked ones.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn make_default(off: &[String]) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let dir = applications_dir().ok_or("no home folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let every = groups().into_iter().flat_map(|(_, exts)| exts).collect::<Vec<_>>();
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=TelescopeView\nGenericName=Gallery Viewer\n\
         Comment=The fastest gallery viewer\nExec=\"{}\" %f\nIcon=telescopeview\nTerminal=false\n\
         Categories=Graphics;Viewer;AudioVideo;\nMimeType={};\n",
        exe.display(),
        mimes(every).join(";")
    );
    std::fs::write(dir.join(DESKTOP), entry).map_err(|e| e.to_string())?;
    // Refreshes the "Open with" lists; not everywhere, and not needed for the defaults.
    let _ = Command::new("update-desktop-database").arg(&dir).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let picked = mimes(chosen(off));
    let ok = Command::new("xdg-mime")
        .arg("default")
        .arg(DESKTOP)
        .args(&picked)
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err("Couldn't run xdg-mime (install xdg-utils)".to_owned());
    }
    Ok(format!("TelescopeView now opens {} file types", picked.len()))
}

/// Whether the everyday types (the ones the user picked) already open with TelescopeView.
/// Without xdg-mime it can't tell, and doesn't ask.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn is_default(off: &[String]) -> bool {
    let picked = EVERYDAY.iter().copied().filter(|e| !off.iter().any(|o| o == e));
    mimes(picked).iter().all(|m| {
        match std::process::Command::new("xdg-mime").args(["query", "default", m]).output() {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim() == DESKTOP,
            _ => true,
        }
    })
}

#[cfg(not(windows))]
pub fn register_if_needed() {}

// --- macOS -----------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub fn make_default(_off: &[String]) -> Result<String, String> {
    Err("On a Mac: Get Info on a file, Open with: TelescopeView, then Change All".to_owned())
}

/// Not offered on macOS yet.
#[cfg(target_os = "macos")]
pub fn is_default(_off: &[String]) -> bool {
    true
}

/// Takes the window off the screen (or puts it back) without hiding it from the app, so it
/// keeps drawing meanwhile. Windows' DWM "cloak".
#[cfg(windows)]
pub fn cloak(window: &impl raw_window_handle::HasWindowHandle, on: bool) {
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let value = i32::from(on);
    // SAFETY: a live window handle, and a pointer to a 4-byte BOOL that outlives the call.
    unsafe {
        let _ = DwmSetWindowAttribute(
            windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _),
            DWMWA_CLOAK,
            std::ptr::from_ref(&value).cast(),
            size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(windows))]
pub fn cloak<T>(_window: &T, _on: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_split_the_types() {
        let all: Vec<_> = groups().into_iter().flat_map(|(_, e)| e).collect();
        let mut seen = all.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), all.len(), "a type sits in two groups");
        assert!(all.contains(&"jpg") && all.contains(&"heic") && all.contains(&"mkv") && all.contains(&"cbz"));
        let off = vec!["png".to_owned(), "mp4".to_owned()];
        let picked = chosen(&off);
        assert!(!picked.contains(&"png") && !picked.contains(&"mp4") && picked.contains(&"jpg"));
        // The everyday ones have a Linux type.
        assert!(EVERYDAY.iter().all(|e| mime(e).is_some()));
    }
}
