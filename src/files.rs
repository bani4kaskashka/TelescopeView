//! Folder and archive listing, sorting, reading pages out of archives, and finding the
//! neighbouring folder or archive (next / previous chapter).
//!
//! A page inside an archive is addressed as `archive path + entry name`, for example
//! `F:\Comics\Vol 1.cbz\012.jpg`. Everything else in the app (cache, rotation, history) just
//! sees paths; only reading the bytes has to know.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const EXTENSIONS: &[&str] = &[
    "png", "apng", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "bmp", "dib", "tif", "tiff", "ico", "cur", "tga",
    "qoi", "pnm", "pbm", "pgm", "ppm", "pam", "hdr", "exr", "dds", "ff", "jxl", "svg", "svgz", "psd",
    // Through the codecs Windows has installed (HEIF and AV1 extensions, Raw Image Extension);
    // HEIC and AVIF also through ffmpeg, without them.
    "heic", "heif", "hif", "avif", "jxr", "wdp", "hdp", "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2",
    "orf", "rw2", "raf", "pef", "srw", "erf", "kdc", "dcr", "mrw", "3fr", "mef", "mos", "x3f", "rwl", "iiq",
];

/// Files that open like a folder: comic archives, PDFs and (image) EPUBs. PDF pages are drawn
/// by Windows' own PDF renderer, RAR needs the `rar` feature (the unrar C++ library).
pub const CONTAINERS: &[&str] = &[
    "cbz",
    "zip",
    #[cfg(feature = "rar")]
    "cbr",
    #[cfg(feature = "rar")]
    "rar",
    "cb7",
    "7z",
    "cbt",
    "tar",
    #[cfg(windows)]
    "pdf",
    "epub",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum SortBy {
    #[default]
    Name,
    Modified,
    Created,
    Size,
    /// By extension, then name.
    Type,
    /// Shuffled, the same way for the whole session.
    Random,
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn ext_in(p: &Path, list: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| list.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub fn ext_lower(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

pub fn is_image(p: &Path) -> bool {
    ext_in(p, EXTENSIONS)
}

/// What a folder shows: images and videos.
pub fn is_page(p: &Path) -> bool {
    is_image(p) || crate::video::is_video(p)
}

pub fn is_container(p: &Path) -> bool {
    ext_in(p, CONTAINERS)
}

/// Pages of a folder or an archive, in reading order.
pub fn list_pages(container: &Path, sort: SortBy, descending: bool) -> std::io::Result<Vec<PathBuf>> {
    if container.is_dir() {
        return list_images(container, sort, descending);
    }
    let (mut entries, ordered) = list_archive(container)?;
    if !ordered {
        // Archives keep no useful dates, so they always go by name (folders inside included).
        entries.sort_by(|a, b| natural_cmp(a, b));
    }
    if descending {
        entries.reverse();
    }
    Ok(entries.into_iter().map(|e| container.join(e)).collect())
}

/// The archive a page lives in and its entry name, or None for a normal file.
pub fn archive_of(page: &Path) -> Option<(&Path, String)> {
    let archive = page.ancestors().skip(1).find(|a| is_container(a) && a.is_file())?;
    let entry = page.strip_prefix(archive).ok()?;
    Some((archive, entry.to_string_lossy().replace('\\', "/")))
}

/// Where a page sits inside its folder or archive, for remembering the reading position.
pub fn relative_name(container: &Path, page: &Path) -> String {
    page.strip_prefix(container).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| file_name(page))
}

/// The bytes of a page, from disk or out of its archive.
/// A photo's camera details for the info panel, if the file has EXIF. Real files only:
/// comic pages don't carry any worth showing.
pub fn read_exif(path: &Path) -> Vec<(&'static str, String)> {
    use exif::{In, Tag, Value};
    let Ok(file) = std::fs::File::open(path) else { return Vec::new() };
    let Ok(data) = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)) else {
        return Vec::new();
    };
    let text = |tag: Tag| {
        let f = data.get_field(tag, In::PRIMARY)?;
        let s = f.display_value().with_unit(&data).to_string();
        let s = s.trim_matches('"').trim().to_owned();
        (!s.is_empty()).then_some(s)
    };
    let mut out = Vec::new();
    let camera = match (text(Tag::Make), text(Tag::Model)) {
        // "Canon" + "Canon EOS R5": the model often repeats the make.
        (Some(make), Some(model)) if model.to_lowercase().starts_with(&make.to_lowercase()) => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => make.or(model),
    };
    if let Some(c) = camera {
        out.push(("Camera", c));
    }
    if let Some(l) = text(Tag::LensModel) {
        out.push(("Lens", l));
    }
    // The first number of a field, whatever type the camera wrote it as.
    let number = |tag: Tag| -> Option<f64> {
        let f = data.get_field(tag, In::PRIMARY)?;
        match &f.value {
            Value::Rational(v) => v.first().map(|r| r.to_f64()),
            Value::SRational(v) => v.first().map(|r| r.to_f64()),
            Value::Short(v) => v.first().map(|&n| n as f64),
            Value::Long(v) => v.first().map(|&n| n as f64),
            _ => None,
        }
        .filter(|n| n.is_finite() && *n > 0.0)
    };
    let mut exposure = Vec::new();
    if let Some(t) = number(Tag::ExposureTime) {
        exposure.push(if t < 1.0 { format!("1/{:.0} s", 1.0 / t) } else { format!("{t:.1} s") });
    }
    if let Some(f) = number(Tag::FNumber) {
        exposure.push(format!("f/{f:.1}"));
    }
    if let Some(iso) = number(Tag::PhotographicSensitivity) {
        exposure.push(format!("ISO {iso:.0}"));
    }
    if let Some(mm) = number(Tag::FocalLength) {
        exposure.push(format!("{mm:.0} mm"));
    }
    if !exposure.is_empty() {
        out.push(("Exposure", exposure.join("  ·  ")));
    }
    if let Some(d) = text(Tag::DateTimeOriginal) {
        out.push(("Taken", d));
    }
    // GPS: degrees, minutes, seconds plus N/S and E/W, as plain decimal degrees.
    let coord = |tag: Tag, reference: Tag, negative: &str| {
        let f = data.get_field(tag, In::PRIMARY)?;
        let Value::Rational(v) = &f.value else { return None };
        let deg = v.iter().take(3).enumerate().map(|(i, r)| r.to_f64() / 60f64.powi(i as i32)).sum::<f64>();
        let sign = if text(reference).is_some_and(|r| r.starts_with(negative)) { -1.0 } else { 1.0 };
        Some(sign * deg)
    };
    if let (Some(lat), Some(lon)) =
        (coord(Tag::GPSLatitude, Tag::GPSLatitudeRef, "S"), coord(Tag::GPSLongitude, Tag::GPSLongitudeRef, "W"))
    {
        out.push(("Location", format!("{lat:.5}, {lon:.5}")));
    }
    out
}

pub fn read_page(page: &Path) -> std::io::Result<Vec<u8>> {
    match std::fs::read(page) {
        Ok(b) => Ok(b),
        Err(e) => match archive_of(page) {
            Some((archive, entry)) => read_entry(archive, &entry),
            None => Err(e),
        },
    }
}

fn io_err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

/// Junk that comes along in archives made on a Mac.
fn is_junk(name: &str) -> bool {
    name.starts_with("__MACOSX/") || name.rsplit('/').next().is_some_and(|n| n.starts_with("._"))
}

fn norm(name: &str) -> String {
    name.replace('\\', "/")
}

/// Image entries of an archive, and whether they are already in reading order (PDF pages,
/// an EPUB's spine) rather than to be sorted by name.
fn list_archive(path: &Path) -> std::io::Result<(Vec<String>, bool)> {
    let names: Vec<String> = match ext_lower(path).as_str() {
        "cbz" | "zip" => {
            let zip = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(io_err)?;
            zip.file_names().map(norm).collect()
        }
        "epub" => {
            let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(io_err)?;
            if let Some(spine) = epub_images(&mut zip) {
                return Ok((spine, true));
            }
            zip.file_names().map(norm).collect()
        }
        #[cfg(feature = "rar")]
        "cbr" | "rar" => unrar::Archive::new(path)
            .open_for_listing()
            .map_err(io_err)?
            .filter_map(|e| e.ok())
            .filter(|e| e.is_file())
            .map(|e| norm(&e.filename.to_string_lossy()))
            .collect(),
        "cb7" | "7z" => {
            let reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).map_err(io_err)?;
            reader.archive().files.iter().filter(|f| !f.is_directory && f.has_stream).map(|f| norm(&f.name)).collect()
        }
        "cbt" | "tar" => {
            let mut tar = tar::Archive::new(std::fs::File::open(path)?);
            let mut out = Vec::new();
            for e in tar.entries_with_seek()? {
                let e = e?;
                if e.header().entry_type().is_file() {
                    out.push(norm(&e.path()?.to_string_lossy()));
                }
            }
            out
        }
        "pdf" => {
            return crate::winimg::pdf_page_count(path).map(|n| ((1..=n).map(|n| format!("Page {n}")).collect(), true));
        }
        _ => return Err(io_err("not an archive")),
    };
    Ok((names.into_iter().filter(|n| !is_junk(n) && is_image(Path::new(n))).collect(), false))
}

/// A solid RAR or 7z (one compressed stream for every page; 7-Zip makes them by default)
/// can't jump to a page: reading page N means unpacking all N before it. So the first read
/// unpacks every page at once and the other reads share that. One archive at a time.
type Unpacked = std::sync::Arc<std::collections::HashMap<String, Vec<u8>>>;
static UNPACKED: std::sync::Mutex<Option<(PathBuf, Result<Unpacked, String>)>> = std::sync::Mutex::new(None);
/// The folder or archive open in the window. Reads still in flight for an archive left behind
/// don't unpack it all again (their pages are thrown away anyway).
static OPEN: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
/// Bigger solid archives are read page by page after all, rather than held in memory.
const UNPACK_ALL_MAX: u64 = 768 << 20;

/// The page from the unpacked archive, or None if this archive isn't read that way.
fn read_unpacked(archive: &Path, entry: &str) -> Option<std::io::Result<Vec<u8>>> {
    let kind = ext_lower(archive);
    if !matches!(kind.as_str(), "cbr" | "rar" | "cb7" | "7z") {
        return None;
    }
    // Other pages' reads wait here while the first one unpacks, then find theirs ready.
    // (Only decode threads take this lock; the UI thread never waits on it.)
    let mut slot = UNPACKED.lock().unwrap_or_else(|e| e.into_inner());
    if slot.as_ref().is_none_or(|(p, _)| p != archive) {
        let open = OPEN.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if open.as_deref().is_some_and(|o| o != archive) {
            return Some(Err(io_err("closed")));
        }
        if std::fs::metadata(archive).ok()?.len() > UNPACK_ALL_MAX || !is_solid(archive) {
            return None;
        }
        // A broken archive is remembered as broken rather than unpacked again for every page.
        let all = unpack_all(archive).map(std::sync::Arc::new).map_err(|e| e.to_string());
        *slot = Some((archive.to_path_buf(), all));
    }
    let all = match &slot.as_ref()?.1 {
        Ok(all) => all.clone(),
        Err(e) => return Some(Err(io_err(e))),
    };
    drop(slot);
    Some(all.get(entry).cloned().ok_or_else(|| io_err("page not found in the archive")))
}

/// Tells the archive reader what's open now, and frees another archive's unpacked pages
/// unless an unpack is still running (then the next archive replaces it).
pub fn set_open(open: &Path) {
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(open.to_path_buf());
    if let Ok(mut slot) = UNPACKED.try_lock()
        && slot.as_ref().is_some_and(|(p, _)| p != open)
    {
        *slot = None;
    }
}

fn is_solid(archive: &Path) -> bool {
    match ext_lower(archive).as_str() {
        #[cfg(feature = "rar")]
        "cbr" | "rar" => unrar::Archive::new(archive).open_for_listing().is_ok_and(|a| a.is_solid()),
        "cb7" | "7z" => sevenz_rust2::Archive::open(archive).is_ok_and(|a| a.is_solid),
        _ => false,
    }
}

fn unpack_all(archive: &Path) -> std::io::Result<std::collections::HashMap<String, Vec<u8>>> {
    let mut out = std::collections::HashMap::new();
    #[cfg(feature = "rar")]
    if matches!(ext_lower(archive).as_str(), "cbr" | "rar") {
        let mut rar = unrar::Archive::new(archive).open_for_processing().map_err(io_err)?;
        while let Some(header) = rar.read_header().map_err(io_err)? {
            let name = norm(&header.entry().filename.to_string_lossy());
            rar = if header.entry().is_file() && is_image(Path::new(&name)) {
                let (data, next) = header.read().map_err(io_err)?;
                out.insert(name, data);
                next
            } else {
                header.skip().map_err(io_err)?
            };
        }
        return Ok(out);
    }
    {
        let mut reader = sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty()).map_err(io_err)?;
        reader
            .for_each_entries(|e, r| {
                if !e.is_directory() && is_image(Path::new(e.name())) {
                    let mut data = Vec::new();
                    r.read_to_end(&mut data)?;
                    out.insert(norm(e.name()), data);
                }
                Ok(true)
            })
            .map_err(io_err)?;
    }
    Ok(out)
}

/// The images of an EPUB in reading order: the spine's documents in turn, and the pictures
/// in each in the order they appear. None if it has no spine or no pictures (a text book).
fn epub_images(zip: &mut zip::ZipArchive<std::fs::File>) -> Option<Vec<String>> {
    let mut text = |name: &str| -> Option<String> {
        use std::io::Read;
        let raw = zip.file_names().find(|n| norm(n) == name)?.to_owned();
        let mut out = String::new();
        zip.by_name(&raw).ok()?.read_to_string(&mut out).ok()?;
        Some(out)
    };
    let container = text("META-INF/container.xml")?;
    let opf_path = xml_tags(&container).find(|(n, _)| *n == "rootfile").and_then(|(_, t)| xml_attr(t, "full-path"))?;
    let opf = text(&opf_path)?;
    let opf_dir = parent_of(&opf_path);
    let mut manifest = std::collections::HashMap::new();
    for (_, t) in xml_tags(&opf).filter(|(n, _)| *n == "item") {
        if let (Some(id), Some(href)) = (xml_attr(t, "id"), xml_attr(t, "href")) {
            manifest.insert(id, (join_href(opf_dir, &href), xml_attr(t, "media-type").unwrap_or_default()));
        }
    }
    let spine: Vec<String> =
        xml_tags(&opf).filter(|(n, _)| *n == "itemref").filter_map(|(_, t)| xml_attr(t, "idref")).collect();
    let mut out: Vec<String> = Vec::new();
    for id in spine {
        let Some((href, kind)) = manifest.get(&id) else {
            continue;
        };
        if kind.starts_with("image/") {
            out.push(href.clone());
            continue;
        }
        let Some(doc) = text(href) else { continue };
        let doc_dir = parent_of(href);
        for (name, t) in xml_tags(&doc) {
            if name == "img" || name == "image" {
                let src = xml_attr(t, "src").or_else(|| xml_attr(t, "xlink:href")).or_else(|| xml_attr(t, "href"));
                if let Some(src) = src {
                    out.push(join_href(doc_dir, &src));
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|n| is_image(Path::new(n)) && seen.insert(n.clone()));
    (!out.is_empty()).then_some(out)
}

/// Every tag in a bit of XML/HTML: (local name without namespace prefix, the tag's text).
fn xml_tags(xml: &str) -> impl Iterator<Item = (&str, &str)> {
    xml.split('<').skip(1).filter_map(|chunk| {
        let tag = &chunk[..chunk.find('>')?];
        let name = tag.split(|c: char| c.is_whitespace() || c == '/').next()?;
        let local = name.rsplit(':').next().unwrap_or(name);
        Some((local, tag))
    })
}

/// An attribute's value, `name="value"` or `name='value'`, entities and %-escapes decoded.
fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(at) = rest.find(name) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + name.len()..].trim_start();
        rest = &rest[at + name.len()..];
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let quote = value.chars().next()?;
        if quote != '"' && quote != '\'' {
            continue;
        }
        let end = value[1..].find(quote)?;
        let raw = value[1..1 + end].replace("&amp;", "&").replace("&quot;", "\"").replace("&apos;", "'");
        return Some(percent_decode(&raw));
    }
    None
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(Ok(v)) = s.get(i + 1..i + 3).map(|h| u8::from_str_radix(h, 16))
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parent_of(entry: &str) -> &str {
    entry.rfind('/').map_or("", |i| &entry[..i])
}

/// A link relative to a folder inside the archive, `..` resolved and `#fragment` dropped.
fn join_href(dir: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or(href);
    let mut parts: Vec<&str> =
        if href.starts_with('/') { Vec::new() } else { dir.split('/').filter(|p| !p.is_empty()).collect() };
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

/// Page number (from 1) of a PDF page's entry name.
pub fn pdf_page_number(entry: &str) -> Option<u32> {
    entry.strip_prefix("Page ")?.parse().ok()
}

fn read_entry(archive: &Path, entry: &str) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let not_found = || io_err("page not found in the archive");
    if let Some(result) = read_unpacked(archive, entry) {
        return result;
    }
    let mut out = Vec::new();
    match ext_lower(archive).as_str() {
        "cbz" | "zip" | "epub" => {
            let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?).map_err(io_err)?;
            // Names were normalised to forward slashes when listing.
            let raw = zip.file_names().find(|n| norm(n) == entry).map(str::to_owned).ok_or_else(not_found)?;
            zip.by_name(&raw).map_err(io_err)?.read_to_end(&mut out)?;
        }
        #[cfg(feature = "rar")]
        "cbr" | "rar" => {
            let mut rar = unrar::Archive::new(archive).open_for_processing().map_err(io_err)?;
            while let Some(header) = rar.read_header().map_err(io_err)? {
                if norm(&header.entry().filename.to_string_lossy()) == entry {
                    return Ok(header.read().map_err(io_err)?.0);
                }
                rar = header.skip().map_err(io_err)?;
            }
            return Err(not_found());
        }
        "cb7" | "7z" => {
            let mut reader =
                sevenz_rust2::ArchiveReader::open(archive, sevenz_rust2::Password::empty()).map_err(io_err)?;
            let raw = reader.archive().files.iter().find(|f| norm(&f.name) == entry).map(|f| f.name.clone());
            out = reader.read_file(&raw.ok_or_else(not_found)?).map_err(io_err)?;
        }
        "cbt" | "tar" => {
            let mut tar = tar::Archive::new(std::fs::File::open(archive)?);
            // Seeks over the pages before it instead of reading through them.
            for e in tar.entries_with_seek()? {
                let mut e = e?;
                if norm(&e.path()?.to_string_lossy()) == entry {
                    e.read_to_end(&mut out)?;
                    return Ok(out);
                }
            }
            return Err(not_found());
        }
        _ => return Err(io_err("not an archive")),
    }
    Ok(out)
}

pub fn list_images(dir: &Path, sort: SortBy, descending: bool) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<Entry> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file() || t.is_symlink()))
        // macOS leaves "._name.jpg" resource files on USB sticks; they aren't images.
        .filter(|e| is_page(&e.path()) && !e.file_name().to_string_lossy().starts_with("._"))
        .map(|e| {
            // On Windows the directory listing already carries dates and sizes, so this is free.
            let meta = e.metadata().ok();
            let time = |t: Option<SystemTime>| t.unwrap_or(SystemTime::UNIX_EPOCH);
            Entry {
                modified: time(meta.as_ref().and_then(|m| m.modified().ok())),
                created: time(meta.as_ref().and_then(|m| m.created().ok())),
                size: meta.as_ref().map_or(0, |m| m.len()),
                path: e.path(),
            }
        })
        .collect();
    sort_files(&mut files, sort);
    if descending {
        files.reverse();
    }
    Ok(files.into_iter().map(|e| e.path).collect())
}

struct Entry {
    path: PathBuf,
    modified: SystemTime,
    created: SystemTime,
    size: u64,
}

fn sort_files(files: &mut [Entry], sort: SortBy) {
    let by_name = |a: &Entry, b: &Entry| natural_cmp(&file_name(&a.path), &file_name(&b.path));
    match sort {
        SortBy::Name => files.sort_by(by_name),
        SortBy::Modified => files.sort_by(|a, b| a.modified.cmp(&b.modified).then_with(|| by_name(a, b))),
        SortBy::Created => files.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| by_name(a, b))),
        SortBy::Size => files.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| by_name(a, b))),
        SortBy::Type => files.sort_by(|a, b| ext_lower(&a.path).cmp(&ext_lower(&b.path)).then_with(|| by_name(a, b))),
        SortBy::Random => {
            // Seeded once per run: re-listing (after a rename, say) keeps the same order.
            static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
            let seed = *SEED.get_or_init(|| {
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64)
            });
            let key = |e: &Entry| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (seed, file_name(&e.path)).hash(&mut h);
                h.finish()
            };
            files.sort_by_cached_key(key);
        }
    }
}

/// True if the folder holds at least one image (stops at the first one).
pub fn has_images(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.filter_map(|e| e.ok()).any(|e| e.file_type().is_ok_and(|t| t.is_file()) && is_page(&e.path()))
    })
}

/// A folder with images in it, or any archive.
/// Whether a folder or archive has anything to show. An archive is listed, so a
/// soundtrack or extras archive between chapters doesn't count as one.
pub fn has_pages(p: &Path) -> bool {
    if p.is_dir() {
        has_images(p)
    } else if ext_lower(p) == "pdf" {
        // Every PDF has pages; counting them means parsing the whole document.
        is_container(p)
    } else {
        is_container(p) && list_archive(p).is_ok_and(|(e, _)| !e.is_empty())
    }
}

/// For a path on a drive that isn't there any more: the drive it moved to (an external drive
/// that came back under another letter), as (old root, new root).
pub fn find_moved_drive(path: &Path) -> Option<(PathBuf, PathBuf)> {
    use std::path::{Component, Prefix};
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return None;
    };
    let (Prefix::Disk(letter) | Prefix::VerbatimDisk(letter)) = prefix.kind() else {
        return None;
    };
    let old = PathBuf::from(format!("{}:\\", letter as char));
    // The drive is still there: the folder was moved or deleted, not the drive re-lettered.
    if old.exists() {
        return None;
    }
    local_drives()
        .into_iter()
        .filter(|&l| l != letter.to_ascii_uppercase())
        .map(|l| PathBuf::from(format!("{}:\\", l as char)))
        .find(|root| swap_drive(path, &old, root).is_some_and(|p| p.exists()))
        .map(|new| (old, new))
}

/// Letters of the drives plugged in right now, without network drives (a disconnected one
/// can take many seconds to answer).
#[cfg(windows)]
fn local_drives() -> Vec<u8> {
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    // SAFETY: no arguments; returns a bit mask of drive letters.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| b'A' + i)
        .filter(|&l| {
            let root: Vec<u16> = format!("{}:\\", l as char).encode_utf16().chain([0]).collect();
            // SAFETY: a valid, NUL-terminated wide string that outlives the call.
            let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
            kind == DRIVE_REMOVABLE || kind == DRIVE_FIXED
        })
        .collect()
}

#[cfg(not(windows))]
fn local_drives() -> Vec<u8> {
    Vec::new()
}

/// `path` moved from under `old` to under `new`, if it was under `old`.
pub fn swap_drive(path: &Path, old: &Path, new: &Path) -> Option<PathBuf> {
    let s = path.to_string_lossy();
    let o = old.to_string_lossy();
    s.get(..o.len()).filter(|head| head.eq_ignore_ascii_case(&o)).map(|_| new.join(&s[o.len()..]))
}

/// Sub-folders and archives directly inside `dir`, in name order. These are the chapters
/// when `dir` is a series.
pub fn child_containers(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) || is_container(&e.path()))
        .map(|e| e.path())
        .collect();
    out.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    out
}

/// The next (or previous) folder or archive next to `dir`, in name order, that has pages.
/// This is "next chapter" for comics stored as one folder or archive per chapter.
pub fn sibling_folder(dir: &Path, forward: bool) -> Option<PathBuf> {
    let parent = dir.parent()?;
    let siblings = child_containers(parent);
    let me = file_name(dir);
    let pos = siblings.iter().position(|d| file_name(d).eq_ignore_ascii_case(&me))?;
    let candidates: Box<dyn Iterator<Item = &PathBuf>> =
        if forward { Box::new(siblings[pos + 1..].iter()) } else { Box::new(siblings[..pos].iter().rev()) };
    // Skips empty or non-image folders (extras, notes), but doesn't wander through hundreds.
    candidates.take(20).find(|d| has_pages(d)).cloned()
}

/// Explorer-style order: case-insensitive, and runs of digits compare as numbers (2 < 10).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| na.len().cmp(&nb.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// "2026-11-04 23:15" in local time, without pulling in a date crate.
pub fn format_time(t: SystemTime) -> String {
    let Ok(d) = t.duration_since(SystemTime::UNIX_EPOCH) else {
        return String::new();
    };
    let secs = d.as_secs() as i64 + local_offset_secs();
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year}-{month:02}-{day:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

#[cfg(windows)]
fn local_offset_secs() -> i64 {
    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: [u16; 8],
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: [u16; 8],
        daylight_bias: i32,
    }
    unsafe extern "system" {
        fn GetTimeZoneInformation(tz: *mut TimeZoneInformation) -> u32;
    }
    // SAFETY: plain out-parameter call into kernel32 with a correctly sized struct.
    let mut tz: TimeZoneInformation = unsafe { std::mem::zeroed() };
    let kind = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = tz.bias + if kind == 2 { tz.daylight_bias } else { tz.standard_bias };
    -(bias as i64) * 60
}

#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    0
}

pub fn format_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["img10.jpg", "IMG2.jpg", "img1.jpg", "a.png", "img02.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a.png", "img1.jpg", "IMG2.jpg", "img02.jpg", "img10.jpg"]);
        assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
    }

    #[test]
    fn sibling_folders_skip_empty_ones() {
        let root = std::env::temp_dir().join(format!("iv-sib-{}", std::process::id()));
        for (d, img) in [("ch1", true), ("ch2", false), ("ch10", true), ("ch3", true)] {
            std::fs::create_dir_all(root.join(d)).unwrap();
            if img {
                std::fs::write(root.join(d).join("p1.jpg"), b"x").unwrap();
            }
        }
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::fs::File::create(root.join("ch4.cbz")).unwrap());
        w.start_file("p1.jpg", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b"x").unwrap();
        w.finish().unwrap();
        // An archive without pictures (a soundtrack) between chapters isn't one.
        let mut w = zip::ZipWriter::new(std::fs::File::create(root.join("ch3 OST.zip")).unwrap());
        w.start_file("track.mp3", zip::write::SimpleFileOptions::default()).unwrap();
        w.finish().unwrap();
        let next = |d: &str, f| sibling_folder(&root.join(d), f).map(|p| file_name(&p));
        assert_eq!(next("ch1", true).as_deref(), Some("ch3"), "ch2 has no images");
        assert_eq!(next("ch3", true).as_deref(), Some("ch4.cbz"), "archives count as chapters, the OST doesn't");
        assert_eq!(next("ch4.cbz", true).as_deref(), Some("ch10"), "natural order");
        assert_eq!(next("ch10", true), None);
        assert_eq!(next("ch3", false).as_deref(), Some("ch1"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn zip_pages_list_and_read() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("iv-zip-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let cbz = root.join("Vol 1.cbz");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&cbz).unwrap());
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for name in ["ch/10.jpg", "ch/2.jpg", "notes.txt", "__MACOSX/ch/._2.jpg", "ch/1.png"] {
            w.start_file(name, opts).unwrap();
            w.write_all(name.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        let pages = list_pages(&cbz, SortBy::Name, false).unwrap();
        let names: Vec<String> = pages.iter().map(|p| archive_of(p).unwrap().1).collect();
        assert_eq!(names, ["ch/1.png", "ch/2.jpg", "ch/10.jpg"]);
        assert_eq!(read_page(&pages[2]).unwrap(), b"ch/10.jpg");
        assert_eq!(relative_name(&cbz, &pages[2]), "ch/10.jpg");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn epub_follows_the_spine() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("iv-epub-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let epub = root.join("Book.epub");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&epub).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        let files: &[(&str, &str)] = &[
            ("mimetype", "application/epub+zip"),
            (
                "META-INF/container.xml",
                r#"<container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            ),
            (
                "OEBPS/content.opf",
                r#"<package><manifest>
                <item id="p1" href="Text/p1.xhtml" media-type="application/xhtml+xml"/>
                <item id="p2" href="Text/p2.xhtml" media-type="application/xhtml+xml"/>
                <item id='cover' href='Images/z%20cover.jpg' media-type='image/jpeg'/>
              </manifest><spine><itemref idref="cover"/><itemref idref="p2"/><itemref idref="p1"/></spine></package>"#,
            ),
            ("OEBPS/Text/p1.xhtml", r#"<html><body><img src="../Images/a.png" alt="x"/></body></html>"#),
            (
                "OEBPS/Text/p2.xhtml",
                r#"<html><body><svg><image xlink:href="../Images/b.jpg#x"/></svg><img src="../Images/a.png"/></body></html>"#,
            ),
            ("OEBPS/Images/a.png", "a"),
            ("OEBPS/Images/b.jpg", "b"),
            ("OEBPS/Images/z cover.jpg", "c"),
        ];
        for (name, body) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        let pages = list_pages(&epub, SortBy::Name, false).unwrap();
        let names: Vec<String> = pages.iter().map(|p| relative_name(&epub, p)).collect();
        // Spine order, not name order; a picture used twice shows once.
        assert_eq!(names, ["OEBPS/Images/z cover.jpg", "OEBPS/Images/b.jpg", "OEBPS/Images/a.png"]);
        assert_eq!(read_page(&pages[1]).unwrap(), b"b");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn seven_zip_pages_read_back() {
        let root = std::env::temp_dir().join(format!("iv-7z-{}", std::process::id()));
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for i in 1..=12 {
            std::fs::write(src.join(format!("{i}.jpg")), format!("page {i}").repeat(500)).unwrap();
        }
        // Solid (7-Zip's default, read through `unpack_all`) and not (page by page).
        let solid = root.join("Solid.cb7");
        let mut w = sevenz_rust2::ArchiveWriter::create(&solid).unwrap();
        w.push_source_path(&src, |_| true).unwrap();
        w.finish().unwrap();
        let plain = root.join("Plain.cb7");
        sevenz_rust2::compress_to_path(&src, &plain).unwrap();
        assert!(is_solid(&solid) && !is_solid(&plain));
        for cb7 in [&solid, &plain] {
            let pages = list_pages(cb7, SortBy::Name, false).unwrap();
            assert_eq!(pages.len(), 12);
            // Out of order, as the preloader asks for them.
            for i in [7, 0, 11, 3] {
                assert_eq!(read_page(&pages[i]).unwrap(), format!("page {}", i + 1).repeat(500).into_bytes());
            }
        }
        set_open(&root);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn hrefs_and_drives() {
        assert_eq!(join_href("OEBPS/Text", "../Images/a.png"), "OEBPS/Images/a.png");
        assert_eq!(join_href("", "./x/y.jpg#frag"), "x/y.jpg");
        assert_eq!(percent_decode("a%20b%zz"), "a b%zz");
        assert_eq!(xml_attr(r#"img data-src="no" src='yes'"#, "src").as_deref(), Some("yes"));
        // Drive letters are a Windows thing.
        #[cfg(windows)]
        {
            let moved = swap_drive(Path::new(r"E:\Comics\Vol 1.cbz"), Path::new(r"e:\"), Path::new(r"F:\"));
            assert_eq!(moved, Some(PathBuf::from(r"F:\Comics\Vol 1.cbz")));
            assert_eq!(swap_drive(Path::new(r"D:\x"), Path::new(r"E:\"), Path::new(r"F:\")), None);
        }
    }

    #[test]
    fn time_formatting() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs((1_700_000_000 - local_offset_secs()) as u64);
        assert_eq!(format_time(t), "2023-11-14 22:13");
    }
}
