# Changelog

All notable changes to TelescopeView are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.1]

### Fixed
- Windows: no more blank white window for a moment when TelescopeView reopens maximized.

## [0.1.0]

The first release.

### Viewing
- Background preloading on a pool of decoder threads: the next pictures are ready before you
  ask for them, and the current one stays up until the next is ready, so nothing flickers.
- Fit to window or width, zoom, zoom lock, enlarge small images, actual size.
- Rotate and flip, saved into the file on request (lossless for JPEG).
- Fullscreen, slideshow, info panel with EXIF (camera, lens, exposure, date taken, GPS).
- Thumbnail panel that slides out from any edge, or stays pinned, or floats.
- Animated GIF, APNG and WebP with pause and frame stepping.
- Videos with sound through ffmpeg, which comes with the Windows downloads and the Linux `.zip`.

### Comics, manga and webtoons
- CBZ, CBR, CB7, CBT, EPUB and (Windows) PDF.
- Webtoon strip mode, two-page spreads, right-to-left reading, split wide pages.
- Chapter-to-chapter reading, bookmarks, `.tvmark` bookmark files, go to page.
- Every folder reopens where you left it.

### Formats
- PNG, APNG, JPEG, WebP, GIF, BMP, TIFF, ICO, TGA, QOI, PNM, HDR, EXR, DDS, Farbfeld,
  JPEG XL, SVG and PSD everywhere.
- On Windows, HEIC / HEIF, AVIF, JPEG XR and camera RAW through the installed Windows codecs.

### Downloads
- Windows installers (per user, no admin, with an uninstaller) and portable zips, for x64 and ARM64.
- macOS: one universal app for Apple silicon and Intel, as a `.dmg` and a `.zip`.
- Linux: portable zips with `install.sh` / `uninstall.sh`, and `.deb` packages, for x64 and ARM64.
- `SHA256SUMS.txt` with every release.

### Interface
- Flat, square, minimal look with dark, light and high-contrast themes.
- Interface size from 100 to 200%, reduce motion, screen-reader names on toolbar buttons.
- Every shortcut can be changed, and every toolbar button shown or hidden.
- One window: files opened while TelescopeView is running show up in the open window.
- Windows "Open with" and Default apps registration, for the current user only.

[Unreleased]: https://github.com/bani4kaskashka/TelescopeView/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/bani4kaskashka/TelescopeView/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/bani4kaskashka/TelescopeView/releases/tag/v0.1.0
