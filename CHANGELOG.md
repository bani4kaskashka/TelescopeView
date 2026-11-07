# Changelog

All notable changes to TelescopeView are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.1.0]

### Added
- Save an enlarged copy, 2x or 4x, in the ... menu. Runs on the CPU, nothing to install.

## [1.0.0]

### Added
- Picture editor: the pencil in the top bar, Shift+E or the ... menu opens it along the bottom. Dials
  for light balance, brightness, exposure, contrast, highlights, shadows, saturation, tint,
  temperature, sharpness, definition and straighten. Crop with free, original and fixed ratios,
  rotate, flip, undo and redo, a before/after Compare line to drag, and Reset. Save replaces the file
  and sends the original to the Recycle Bin; Save a copy writes a new one. EXIF and the color profile
  are kept, and anything that would lose your changes asks first.
- Find duplicates: the ... menu scans a folder for exact copies and look-alikes and shows them in
  pairs to keep or delete. Nothing is deleted until you confirm on the last page, and it goes to the
  Recycle Bin.
- Pick a frame: on a video, the ... menu or a right-click splits it into every frame as a grid,
  starting from where the video was. Save any frame at full resolution as PNG or JPEG. It never
  overwrites a file.
- Glide: an optional slide from one image to the next, with adjustable time. Off by default.
- Side arrows: a small triangle on the side the mouse is on, black or white depending on what's
  under it.
- Find duplicates and Pick a frame can each get an optional toolbar button in Settings, Toolbar.

### Changed
- The bottom page bar is a little bigger and shows which image you're hovering sooner.

## [0.1.4]

### Fixed
- If TelescopeView can't start drawing at all, it now shows a message saying why, including when the
  graphics library crashes while starting. Before, the app could just disappear. The message lists
  both errors, and they also go to stderr.

## [0.1.3]

### Fixed
- TelescopeView now opens on a PC without OpenGL 2, like a virtual machine with no graphics driver,
  by switching to its other way of drawing. If neither works, it says so instead of closing silently.

## [0.1.2]

### Fixed
- Windows: TelescopeView now starts on a PC without the Microsoft C++ runtime. Before, it stopped
  with "MSVCP140.dll was not found", and the installer hung waiting on that message.

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

[Unreleased]: https://github.com/bani4kaskashka/TelescopeView/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/bani4kaskashka/TelescopeView/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/bani4kaskashka/TelescopeView/compare/v0.1.4...v1.0.0
[0.1.4]: https://github.com/bani4kaskashka/TelescopeView/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/bani4kaskashka/TelescopeView/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/bani4kaskashka/TelescopeView/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/bani4kaskashka/TelescopeView/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/bani4kaskashka/TelescopeView/releases/tag/v0.1.0
