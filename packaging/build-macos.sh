#!/bin/sh
# Builds TelescopeView.app (one universal binary for Apple silicon and Intel), then:
#   TelescopeView-<version>-macos-universal.dmg  (installer: drag the app onto Applications)
#   TelescopeView-<version>-macos-universal.zip  (just the app)
# Run on a Mac with Rust installed:  sh packaging/build-macos.sh
# The app isn't signed with an Apple Developer ID: the first time, right-click it and pick Open.
# Videos need ffmpeg from Homebrew (brew install ffmpeg).
set -eu
cd "$(dirname "$0")/.."
# rust-toolchain.toml pins the Windows (MSVC) toolchain for the Windows build; not here.
export RUSTUP_TOOLCHAIN=stable

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

out=target/macos
app="$out/TelescopeView.app"
name="TelescopeView-$version-macos-universal"
rm -rf "$out"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/TelescopeView" \
    target/aarch64-apple-darwin/release/TelescopeView \
    target/x86_64-apple-darwin/release/TelescopeView
sed "s/VERSION/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

# Icon: every size macOS asks for, from the PNGs in packaging/icons.
iconset="$out/AppIcon.iconset"
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
    cp "packaging/icons/$s.png" "$iconset/icon_${s}x${s}.png"
    cp "packaging/icons/$((s * 2)).png" "$iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"

# Ad-hoc signature: enough for Apple silicon to run it (Gatekeeper still asks once).
codesign --force --deep --sign - "$app"

# The .dmg: the app next to a link to Applications, to drag it across.
stage="$out/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname TelescopeView -srcfolder "$stage" -ov -format UDZO "$out/$name.dmg"
rm -rf "$stage"
echo "Built $out/$name.dmg"

# The .zip keeps the app's signature and permissions (ditto, not zip).
ditto -c -k --keepParent "$app" "$out/$name.zip"
echo "Built $out/$name.zip"
