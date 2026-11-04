# Contributing to TelescopeView

Thanks for helping out. Bug reports, format requests and pull requests are all welcome.

## Reporting a bug

Open an issue with the **Bug report** template. The most useful things to include:

- your OS and version, and the TelescopeView version (from the release you downloaded),
- the file type you were viewing (and a sample file, if you can share one),
- what you did, what you expected, and what happened instead.

If TelescopeView felt slow, run it with `IV_STATS=1` set and paste the lines it prints: they
show for every step whether the picture was ready in time.

## Building

```sh
cargo build --release
cargo test
```

The Windows build uses the MSVC toolchain pinned in `rust-toolchain.toml`. On macOS and Linux,
set `RUSTUP_TOOLCHAIN=stable` (the packaging scripts and CI already do). RAR support compiles
the UnRAR C++ library; `--no-default-features` builds without it.

Debug builds still optimize dependencies, because image decoding is unusable without it, so the
first build takes a while.

## Project layout

| Path | What's there |
|---|---|
| `src/main.rs` | Startup, window options, renderer choice |
| `src/app.rs` | App state, navigation and the preload cache |
| `src/loader.rs` | Background decoding: worker threads on a priority queue |
| `src/canvas.rs` | The image itself: fit, zoom, pan, rotation, animation, the webtoon strip |
| `src/ui.rs` | Everything around the image: toolbar, page bar, menus, overlays, Settings |
| `src/thumbs.rs` | The thumbnail panel |
| `src/keys.rs` | The shortcut table: every action and its default keys |
| `src/files.rs` | Folder and archive listing, sorting, next / previous chapter |
| `src/actions.rs` | File actions: delete, rename, copy / move, save rotation, set as wallpaper |
| `src/video.rs` | Video playback through ffmpeg |
| `src/winimg.rs` | Windows codecs (HEIC, AVIF, RAW, JPEG XR) and PDF pages |
| `src/system.rs` | Windows "Open with" / Default apps registration |
| `src/instance.rs` | One window: hands newly opened files to the running window |
| `src/theme.rs` | Colours, icons and the app icon |
| `assets/` | The app icon (`app_icon.svg`, `app.ico`) and the toolbar icons |
| `packaging/` | macOS and Linux build scripts, `.desktop` file, `Info.plist`, PNG icons |
| `docs/` | The user guide and the README images |

## Conventions

- **Speed first.** Never block the UI thread on decoding or disk access. Anything new that reads
  files goes through the loader or its own thread.
- **The look is flat and square.** No rounded corners, no shadows, colours from `theme.rs`.
  Toolbar icons are minimal 24 × 24 SVGs with square line caps (see `assets/icons/`).
- **Keep the toolbar small.** New features go in the view menu, the **...** menu or Settings,
  not as new default toolbar buttons.
- **Shortcuts live in one table.** Add new key-driven actions to `DEFS` in `src/keys.rs`, so the
  help panel, tooltips and Settings pick them up.
- **Never rename a settings id.** The ids in `keys.rs` and the settings file are saved on users'
  machines.
- Run `cargo fmt` (the project's `rustfmt.toml` sets a 120-column width) and `cargo test` before
  sending a pull request.

## Changing the app icon

Edit `assets/app_icon.svg`, then regenerate the exe icon and the package icons:

```sh
cargo test write_app_ico -- --ignored
cargo test write_icon_pngs -- --ignored
```

## Pull requests

- One change per pull request, with a short description of what it does and how you tested it.
- Update [docs/GUIDE.md](docs/GUIDE.md) when you add or change a feature, and add a line under
  **Unreleased** in [CHANGELOG.md](CHANGELOG.md).
- By submitting a pull request, you agree that your contribution is dual-licensed under the
  MIT and Apache-2.0 licenses, like the rest of the project.
