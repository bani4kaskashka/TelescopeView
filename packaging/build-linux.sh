#!/bin/sh
# Builds the Linux downloads for one architecture:
#   TelescopeView-<version>-linux-<x64|arm64>.zip  (unzip anywhere and run, or ./install.sh)
#   telescopeview_<version>_<amd64|arm64>.deb      (with cargo-deb installed)
#
#   sh packaging/build-linux.sh                                     # this machine's architecture
#   TARGET=aarch64-unknown-linux-gnu sh packaging/build-linux.sh    # ARM64 (through `cross`)
#
# Needs Rust, a C++ compiler (CBR support), zip, and on Debian/Ubuntu: libxkbcommon-dev
# libgl1-mesa-dev libasound2-dev. Building for another architecture uses `cross`
# (cargo install cross, plus Docker or Podman); see Cross.toml.
# BUNDLE_FFMPEG=1 puts a portable ffmpeg (for videos) into the zip; the .deb depends on the
# distribution's ffmpeg package instead.
set -eu
cd "$(dirname "$0")/.."
# rust-toolchain.toml pins the Windows (MSVC) toolchain for the Windows build; not here.
export RUSTUP_TOOLCHAIN=stable

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
target="${TARGET:-}"
if [ -n "$target" ]; then
    arch="${target%%-*}"
    if [ "$arch" = "$(uname -m)" ]; then
        cargo build --release --target "$target"
    else
        cross build --release --target "$target"
    fi
    bin="target/$target/release/TelescopeView"
else
    arch="$(uname -m)"
    cargo build --release
    bin="target/release/TelescopeView"
fi
case "$arch" in
    x86_64) label=x64 ff=linux64 ;;
    aarch64) label=arm64 ff=linuxarm64 ;;
    *) label="$arch" ff="" ;;
esac

name="TelescopeView-$version-linux-$label"
out="target/linux/$name"
rm -rf "$out" "target/linux/$name.zip"
mkdir -p "$out/icons"
cp "$bin" "$out/telescopeview"
cp packaging/linux/telescopeview.desktop "$out/"
cp packaging/icons/32.png packaging/icons/64.png packaging/icons/128.png packaging/icons/256.png "$out/icons/"

if [ "${BUNDLE_FFMPEG:-0}" = 1 ]; then
    [ -n "$ff" ] || { echo "no portable ffmpeg for $arch" >&2; exit 1; }
    base=https://github.com/BtbN/FFmpeg-Builds/releases/download/latest
    file="ffmpeg-n9.0-latest-$ff-lgpl-shared-9.0.tar.xz"
    dl=target/ffmpeg-dl
    rm -rf "$dl" && mkdir -p "$dl"
    curl -fsSL -o "$dl/$file" "$base/$file"
    curl -fsSL -o "$dl/checksums.sha256" "$base/checksums.sha256"
    (cd "$dl" && grep " $file\$" checksums.sha256 | sha256sum -c -)
    tar -xJf "$dl/$file" -C "$dl"
    src="$dl/${file%.tar.xz}"
    mkdir -p "$out/ffmpeg/bin" "$out/ffmpeg/lib"
    cp "$src/bin/ffmpeg" "$src/bin/ffprobe" "$out/ffmpeg/bin/"
    cp -P "$src"/lib/*.so* "$out/ffmpeg/lib/"
    cp "$src/LICENSE.txt" "$out/ffmpeg/LICENSE.txt"
    echo "FFmpeg ($file, LGPL 2.1 or later), from https://github.com/BtbN/FFmpeg-Builds. Source code: https://ffmpeg.org/download.html" > "$out/ffmpeg/README.txt"
fi

cat > "$out/install.sh" <<'EOF'
#!/bin/sh
# Installs TelescopeView for the current user: the program (with its ffmpeg), a menu entry,
# and "Open with" for pictures, videos and comics. ./uninstall.sh takes it all out again.
set -eu
cd "$(dirname "$0")"
app="$HOME/.local/lib/telescopeview"
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$app"
install -m 755 telescopeview "$app/telescopeview"
if [ -d ffmpeg ]; then rm -rf "$app/ffmpeg" && cp -R ffmpeg "$app/"; fi
ln -sf "$app/telescopeview" "$HOME/.local/bin/telescopeview"
# The menu entry starts it by its full path (~/.local/bin isn't always on the desktop's PATH).
sed "s|^Exec=.*|Exec=\"$app/telescopeview\" %f|" telescopeview.desktop > "$HOME/.local/share/applications/telescopeview.desktop"
for s in 32 64 128 256; do
    mkdir -p "$HOME/.local/share/icons/hicolor/${s}x${s}/apps"
    install -m 644 "icons/$s.png" "$HOME/.local/share/icons/hicolor/${s}x${s}/apps/telescopeview.png"
done
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "Installed. Start it from the menu, or run: telescopeview"
echo "To open your files with it: ... menu > Make default."
EOF

cat > "$out/uninstall.sh" <<'EOF'
#!/bin/sh
# Removes what install.sh put in place (your settings in ~/.local/share/telescopeview stay).
set -eu
rm -rf "$HOME/.local/lib/telescopeview"
rm -f "$HOME/.local/bin/telescopeview" "$HOME/.local/share/applications/telescopeview.desktop"
for s in 32 64 128 256; do
    rm -f "$HOME/.local/share/icons/hicolor/${s}x${s}/apps/telescopeview.png"
done
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "Uninstalled."
EOF
chmod +x "$out/install.sh" "$out/uninstall.sh" "$out/telescopeview"
(cd target/linux && zip -qry "$name.zip" "$name")
echo "Built target/linux/$name.zip"

if command -v cargo-deb >/dev/null 2>&1; then
    if [ -n "$target" ]; then
        cargo deb --no-build --no-strip --target "$target" --output target/linux/
    else
        cargo deb --no-build --output target/linux/
    fi
else
    echo "(cargo install cargo-deb for the .deb too)"
fi
