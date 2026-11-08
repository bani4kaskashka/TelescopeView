# TelescopeView user guide

Everything TelescopeView opens and does, with every default shortcut. Press H in the app for
the shortcut list, and change any key in Settings, Shortcuts.

## What it opens

- **Images:** PNG, APNG, JPEG, WebP, GIF (animated too), BMP, TIFF, ICO, TGA, QOI, PNM, HDR,
  EXR, DDS, Farbfeld, JPEG XL, SVG, PSD.
  On Windows also anything with an installed Windows codec: HEIC / HEIF, AVIF, JPEG XR and
  camera RAW (DNG, CR2, CR3, NEF, ARW, ...). HEIC / HEIF and AVIF also open through ffmpeg
  when Windows has no codec for them (or on Linux).
- **Comics and books:** CBZ / ZIP, CBR / RAR, CB7 / 7Z, CBT / TAR, EPUB (picture books and comics,
  in the book's own page order), PDF (Windows).
- **Videos:** anything ffmpeg plays (MP4, MKV, WebM, MOV, AVI, WMV, HEVC / H.265 including bare
  `.hevc` / `.h265` streams, ...), with sound. The Windows and Linux downloads carry their own
  ffmpeg (an LGPL build from BtbN/FFmpeg-Builds, in `ffmpeg/` next to the app), so nothing needs
  installing; the .deb pulls in the distribution's ffmpeg. Otherwise (the Mac, a bare exe) it
  uses an installed ffmpeg (`brew install ffmpeg`, `winget install ffmpeg`). Videos sit in the
  folder between the pictures and get their first frame as the thumbnail.

## Viewing

The toolbar keeps to the everyday buttons: open, previous / next, rotate, the view menu, zoom,
page number, bookmark, fullscreen, settings and the ... menu. The view menu (its icon shows the
layout in use) has fit, webtoon strip, two pages, right to left, split wide pages, enlarge,
zoom lock, thumbnails and the slideshow. Settings, Toolbar puts any of those back as a button.

- **Next / previous:** arrows, A / D, PgUp / PgDn, mouse back / forward, or click the
  left / right third of the picture. The wheel zooms around the cursor (Settings, Viewing can make
  it scroll and turn pages instead; on a video it always zooms). The thin bar at the bottom jumps anywhere (click or drag).
- **Fit and zoom:** F fits the window, W fits the width, E enlarges small images. The zoom
  slider and percentage sit in the toolbar (click the percentage for actual size and back);
  Ctrl+wheel or + / − zoom too, 1 is actual size.
- **Zoom lock:** L (or the view menu) keeps the zoom and the spot you zoomed into
  for the next images, handy for comparing shots.
- **Fullscreen** (F11, Enter, double-click), **slideshow** (S, speed in the view menu),
  **rotate** (R / Shift+R), **flip** (X left to right, Y upside down), **info** (I).
- **Info panel:** size, format, file date, and for photos the camera, lens, exposure, date taken
  and GPS location from the EXIF.
- **Animations** (GIF, APNG, WebP): K pauses, `,` and `.` step frame by frame.
- **Videos:** play on their own; the ones either side are opened ahead, so moving to them starts
  at once (loading dots while one still starts). Space or K (or a click in the middle) plays /
  pauses, `,` and `.` jump 5 seconds, and the bar along the bottom (shown while the mouse moves or it's paused)
  has the seek bar, mute and volume.
- **Thumbnails** (N, or the view menu): every picture in the folder in a strip that slides out
  when the mouse touches its edge. The pin in its corner keeps it on screen; drag the panel to
  any side of the window to move it there. Right-click it for size, a floating panel and the
  rest. Hovering the bottom
  bar previews the page under the mouse.
- **Files:** copy image / path / the file itself (Ctrl+Shift+F, pastes into Explorer), show in Explorer, rename (F2), delete to the Recycle Bin (Del),
  save the rotation or flip into the file (Ctrl+S, lossless for JPEG), copy / move to a folder, set as
  desktop background or (Windows) lock screen from the ... menu's "Set as", and on Windows Open with, Edit, Print and Properties.
- **Editing:** the pencil in the top bar, Shift+E or the ... menu opens the editor along the
  bottom. Drag a dial right or up to raise it, double-click to zero it, or click it and use the
  arrow keys: light balance, brightness, exposure, contrast, highlights, shadows, saturation,
  tint, temperature, sharpness, definition and straighten. A dial shows its icon, and its number
  while hovered or just changed. The crop tool has free, original and fixed ratios (Enter or the
  crop button applies it); there are also rotate, flip, undo / redo (Ctrl+Z, Ctrl+Y), Compare (a
  line to drag over the picture, before on its left and after on its right) and Reset. Save
  (Ctrl+S) replaces the file and sends the original to the Recycle Bin; Save a copy writes a new
  one. Esc leaves the editor. Anything that would lose your changes (Esc, opening something else,
  closing the window) asks first. EXIF and the color profile are kept.
- **Find duplicates:** the ... menu asks for a folder (the open one to start with) and how alike
  pictures have to be, then shows the look-alikes in pairs: left arrow keeps the left, right arrow
  the right, down or Space both, Del deletes both, S keeps the rest of a set, Z undoes, Esc leaves.
  The wheel zooms both pictures together, drag pans, double-click resets. Nothing is deleted until
  "Move to Recycle Bin" on the last page. Exact copies always show; moving the slider never scans
  again. Rotated or mirrored copies aren't found, and crops need a lower slider (about 75 for 3%
  off each side). Settings, Toolbar can give it a button.
- **Pick a frame:** on a video, the ... menu (or a right-click) splits it into every frame as a
  grid, starting from where the video was. Arrows, Page Up/Down, Home and End move around, or
  click or drag the slider. Enter or a double-click saves that frame at full resolution as PNG or
  JPEG next to the video, or in a folder you choose, and never over another file. Ctrl+Shift+S
  saves as, Stop keeps the frames so far, Esc closes. Long videos take memory while it's open,
  roughly 10 MB per 1000 frames. Settings, Toolbar can give it a button.
- **Save an enlarged copy:** on a picture, the ... menu (or a right-click) saves a copy at 2x or 4x
  the size next to it as `name 2x.jpg`, in the same format where it can, turned the way it is on
  screen. The original is never touched and nothing is written over. It runs on the CPU with
  nothing to install or download. It helps most on small pictures: from about 1000 px up the
  difference is hard to see, and JPEG blocks stay. Not for pages inside archives or animations.
- **Select text** (Shift+T, or the ... menu / a right-click): reads the words in the pages on
  screen, comic and webtoon pages and a paused video frame included, and shows them faintly.
  Drag across the text to pick it letter by letter, click one word, double-click a line, Ctrl+A for the whole page,
  then Ctrl+T (or Copy text in the menu; Ctrl+C still copies the picture). While it's on, the
  left button picks words instead of panning or turning pages; the middle button still pans.
  Esc drops what's picked, Esc again stops. On a picture that fits the window (nothing to pan)
  you can also just hold the left button on it for a moment and drag: the words light up as you
  go, and a click off the words ends it. It works offline: Windows' own text recognition (the languages installed in Windows
  Settings), Apple's Vision on a Mac (macOS 10.15 or later), and on Linux the ocrs models that
  come with the download (Latin letters only).
- **Glide:** Settings, Viewing can slide from one image to the next, with the time it takes
  (off by default, and off with Reduce motion).
- **Side arrows:** with click zones on, a small triangle shows on the side the mouse is on, black
  or white depending on what's under it. It hides when the mouse rests.
- **Position bar:** the tiny bookmarks above it are your bookmarked pages.
- **Resume:** every folder reopens where you left it. If an external drive comes back under
  another letter, the saved places follow it.
- **Paste** (Ctrl+V): opens a copied file or path, or a copied picture (saved as a PNG first).
- **One window:** opening another file while TelescopeView is up shows it in the open window
  (can be turned off in Settings, Window).

H shows every shortcut. Every one of them can be changed in Settings.

## Comics, manga and webtoons

- **Chapters:** a series folder (chapters inside) continues with the chapter read last. Past the
  last page, the same move again opens the next folder or archive; `]` and `[` jump straight there.
- **Webtoons:** V turns the folder into one long strip. + / − set the column width, S auto-scrolls.
- **Books:** P shows two pages side by side (cover alone, wide double-page scans alone).
  M reads right to left (manga), and the arrows and page bar follow. `,` and `.` shift the
  pairs by one page. The layout is remembered per series.
- **Bookmarks:** B bookmarks the page (a tick on the page bar, the button fills in). The
  bookmark button's right-click menu, the ... menu, and the start screen list them.
- **Bookmark files:** bookmarking a page (B, the button, or Ctrl+B for just the file) also asks
  where to save it as a small `.tvmark` file; the dialog starts where the last one went.
  Cancelling it leaves the page unbookmarked. Double-click it, open it or drop it on the window
  to carry on from that page.
- **Split wide pages** (J): a double-page scan is read a half at a time, in reading order.
- **Go to page:** G, or click the page counter.

## Settings

The sliders button in the toolbar (or Ctrl+,) opens Settings: the sections down the left, the
chosen one on the right.

- **Viewing:** fit, zoom step, enlarge small images, zoom lock, mouse wheel turns pages instead
  of zooming, click zones, arrow keys look around a zoomed-in image first, sharp pixels.
- **Folders and order:** what happens at the last image, reopening where you left off, sort order.
- **Comics:** two pages, right to left, split wide pages.
- **Thumbnails:** on or off, only at the edge, where, size, preview on the bottom bar.
- **Slideshow:** seconds per image, stop at the end.
- **Look:** theme, background, checkerboard, interface size, high contrast, reduce motion,
  position bar, info panel.
- **Window:** start in fullscreen, Esc closes, bars in fullscreen, page number in the corner,
  one window for every file opened.
- **Files:** ask before deleting, clear the Continue list.
- **Performance:** how many images are kept ready ahead, decoding threads, thumbnail threads.
- **Toolbar:** show or hide each button (the view menu ones can get buttons of their own too).
- **Shortcuts:** change any key, add a second one, or go back to the default. A key taken from
  another action moves over (and says so).

Also: F5 reloads the folder, 0 fits, 1 is actual size, Backspace goes back, Ctrl+W closes, and the
... menu has a Chapters list for jumping between the folders of a series.

## Accessibility

In the ... menu: interface size 100–200% (or Ctrl + / Ctrl −), high contrast (pure black and
white, stronger lines), reduce motion (no eased scrolling), and keep the bars in fullscreen.
Toolbar buttons carry names for screen readers.

## Default app

... menu → Make default (or Settings, Window) opens a small card: tick what TelescopeView should
open (Pictures, RAW photos, Videos, Comics), or "Pick formats…" to leave single formats, say
PNG, to another app. Unticked types stay with whatever opens them now. Until it's the default,
the same card comes up by itself at most once a week ("Don't ask again", or switch it off in
Settings, Window).

- **Windows:** doesn't let an app make itself the default, so Make default registers the ticked
  types and opens Settings at TelescopeView's own Default apps page, which lists only those:
  pick it for each type there. It registers itself for "Open with" and for `.tvmark` bookmark
  files on first start, or straight away from the installer (current user only, no admin);
  uninstalling takes that back out. By hand: `TelescopeView.exe --register` / `--unregister`.
  The Microsoft Store version has its types built in, so there's nothing to tick or register:
  Make default just opens that page. It leaves plain `.zip`, `.rar`, `.7z`, `.tar`, `.pdf` and
  `.epub` out of "Open with"; they still open from inside the app or by dropping them on it.
- **Linux:** Make default writes `~/.local/share/applications/telescopeview.desktop` and sets it
  as the default for the ticked types with `xdg-mime` (needs xdg-utils; GNOME and KDE both
  follow it). `install.sh` from the .zip (or the .deb) adds the menu entry and "Open with" for
  pictures, videos and comics.
- **macOS:** open files from inside the app or drop them on the window. Finder's "Open With"
  isn't wired up yet (macOS hands the file over through an Apple Event the window library
  doesn't pass on).

## Downloads

Every release has, for Intel/AMD (x64) and ARM64 alike:

| | Install | Portable |
|---|---|---|
| **Windows** | `-windows-x64-setup.exe` / `-windows-arm64-setup.exe`: installs for the current user (no admin), Start menu entry, uninstaller | `-windows-x64.zip` / `-windows-arm64.zip`: unzip anywhere and run `TelescopeView.exe` |
| **macOS** | `-macos-universal.dmg`: drag the app onto Applications (one app for Apple silicon and Intel) | `-macos-universal.zip`: just the app |
| **Linux** | `telescopeview_<version>-1_amd64.deb` / `_arm64.deb` | `-linux-x64.zip` / `-linux-arm64.zip`: run `telescopeview`, or `./install.sh` for a menu entry (`./uninstall.sh` removes it) |

The Windows and Linux downloads (not the .deb) carry their own ffmpeg for videos. The Windows
installer and the Mac app aren't signed with a paid certificate yet: Windows SmartScreen asks
"Run anyway" once, and on a Mac right-click the app and pick Open the first time. Each release
also lists the SHA-256 checksum of every file in `SHA256SUMS.txt`.

Building them yourself: `packaging/build-windows.ps1 -Arch x64|arm64` (installer with Inno
Setup 6), `sh packaging/build-linux.sh` (`TARGET=aarch64-unknown-linux-gnu` for ARM64, through
`cross`), `sh packaging/build-macos.sh` on a Mac. `.github/workflows/build.yml` runs all of them.

## Known limits

- PDF pages need Windows (it uses the PDF reader built into Windows).
- EPUB: picture books and comics only. Text-only novels have no pages to show.
- macOS: Finder's "Open With" (see above). On a Mac, Cmd+Backspace deletes.
- The Mac, Linux and Windows ARM64 builds compile but haven't been tried on a real Mac, Linux
  PC or ARM64 Windows PC yet.
- Videos on the Mac need ffmpeg from Homebrew (`brew install ffmpeg`).
- Select text on Linux reads Latin letters only (no Cyrillic, Greek or Asian scripts), and needs
  the `ocr` folder from the download; built from source, put the two models from
  `packaging/build-linux.sh` in an `ocr` folder next to the program.
- Editing: animations and videos can't be edited. Saved files are 8-bit per channel, WebP is
  written lossless, and pictures over 64 MP load scaled down, so they can only be saved as a
  smaller copy.
