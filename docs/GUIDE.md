# TelescopeView user guide

Everything TelescopeView opens and does, with every default shortcut. Press H in the app for
the shortcut list, and change any key in Settings, Shortcuts.

## What it opens

- **Images:** PNG, APNG, JPEG, WebP, GIF (animated too), BMP, TIFF, ICO, TGA, QOI, PNM, HDR,
  EXR, DDS, Farbfeld, JPEG XL, SVG, PSD.
  On Windows also anything with an installed Windows codec: HEIC / HEIF, AVIF, JPEG XR and
  camera RAW (DNG, CR2, CR3, NEF, ARW, ...).
- **Comics and books:** CBZ / ZIP, CBR / RAR, CB7 / 7Z, CBT / TAR, EPUB (picture books and comics,
  in the book's own page order), PDF (Windows).
- **Videos:** anything ffmpeg plays (MP4, MKV, WebM, MOV, AVI, WMV, ...), with sound. Needs
  ffmpeg on the PATH (Windows: `winget install ffmpeg`). Videos sit in the folder between the
  pictures and get a poster frame as their thumbnail.

## Viewing

The toolbar keeps to the everyday buttons: open, previous / next, rotate, the view menu, zoom,
page number, bookmark, fullscreen, settings and the ... menu. The view menu (its icon shows the
layout in use) has fit, webtoon strip, two pages, right to left, split wide pages, enlarge,
zoom lock, thumbnails and the slideshow. Settings, Toolbar puts any of those back as a button.

- **Next / previous:** arrows, A / D, PgUp / PgDn, mouse back / forward, the wheel, or click the
  left / right third of the picture. The thin bar at the bottom jumps anywhere (click or drag).
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
- **Videos:** play on their own. Space or K (or a click in the middle) plays / pauses, `,` and
  `.` jump 5 seconds, and the bar along the bottom (shown while the mouse moves or it's paused)
  has the seek bar, mute and volume.
- **Thumbnails** (N, or the view menu): every picture in the folder in a strip that slides out
  when the mouse touches its edge. The pin in its corner keeps it on screen; drag the panel to
  any side of the window to move it there. Right-click it for size, a floating panel and the
  rest. Hovering the bottom
  bar previews the page under the mouse.
- **Files:** copy image / path / the file itself (Ctrl+Shift+F, pastes into Explorer), show in Explorer, rename (F2), delete to the Recycle Bin (Del),
  save the rotation or flip into the file (Ctrl+S, lossless for JPEG), copy / move to a folder, set as
  desktop background or (Windows) lock screen from the ... menu's "Set as", and on Windows Open with, Edit, Print and Properties.
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
  where to save it as a small `.tvmark` file; the dialog starts where the last one went. Double-click it, open it or drop it on the window
  to carry on from that page.
- **Split wide pages** (J): a double-page scan is read a half at a time, in reading order.
- **Go to page:** G, or click the page counter.

## Settings

The sliders button in the toolbar (or Ctrl+,) opens Settings: the sections down the left, the
chosen one on the right.

- **Viewing:** fit, zoom step, enlarge small images, zoom lock, mouse wheel zooms instead of
  turning, click zones, arrow keys look around a zoomed-in image first, sharp pixels.
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

- **Windows:** ... menu → Make default image viewer opens Default apps at TelescopeView; pick it
  for each type there (Windows doesn't let an app make itself the default). Until
  it's the default, a small card offers this at most once a week ("Don't ask again", or switch it
  off in Settings, Window). It registers itself for "Open with" and for `.tvmark` bookmark files
  on first start (current user only, no admin).
- **Linux:** `install.sh` from the .tar.gz (or the .deb) adds the menu entry and "Open with" for
  images and comics.
- **macOS:** open files from inside the app or drop them on the window. Finder's "Open With"
  isn't wired up yet (macOS hands the file over through an Apple Event the window library
  doesn't pass on).

## Known limits

- PDF pages need Windows (it uses the PDF reader built into Windows).
- EPUB: picture books and comics only. Text-only novels have no pages to show.
- macOS: Finder's "Open With" (see above). On a Mac, Cmd+Backspace deletes.
- The Mac and Linux builds compile but haven't been tried on a real Mac or Linux PC yet.
