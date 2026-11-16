#!/usr/bin/env bash
# Build a universal (Apple Silicon + Intel) macOS DMG for Pinto Slicer.
#
# The bundle is assembled on an Apple Silicon runner: rustc cross-compiles the
# Intel slice with the same SDK and lipo joins the two binaries, so one DMG
# serves every Mac. The result is ad-hoc signed, not notarized; see
# RELEASING.md for what users see on first launch.
#
# Usage: packaging/build-macos.sh [output_dir]
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${1:-"$root/dist"}
app_id=io.github.makzumi.pinto-slicer
app_name="Pinto Slicer"
targets=(aarch64-apple-darwin x86_64-apple-darwin)

[[ $(uname -s) == Darwin ]] || { echo "This script must run on macOS." >&2; exit 1; }
for tool in sips iconutil hdiutil lipo codesign plutil shasum; do
    command -v "$tool" >/dev/null 2>&1 || { echo "Required tool is missing: $tool" >&2; exit 1; }
done

version=$(awk -F '"' '/^version = / { print $2; exit }' "$root/Cargo.toml")
[[ -n $version ]] || { echo "Could not parse version from Cargo.toml" >&2; exit 1; }
[[ $output_dir = /* ]] || output_dir="$root/$output_dir"

echo "Building $app_name $version for aarch64 and x86_64..."
rustup target add "${targets[@]}"

cd "$root"
for target in "${targets[@]}"; do
    cargo build --release --locked --target "$target"
done

bundle="$root/build/macos/$app_name.app"
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"

# Strip each slice, then join them into one universal executable. lipo drops
# any existing code signature, so the bundle is signed again further down.
slices=()
for target in "${targets[@]}"; do
    slice="$root/build/macos/pinto_slicer-$target"
    install -m755 "$root/target/$target/release/pinto_slicer" "$slice"
    strip -x "$slice"
    slices+=("$slice")
done
lipo -create "${slices[@]}" -output "$bundle/Contents/MacOS/pinto_slicer"
chmod 755 "$bundle/Contents/MacOS/pinto_slicer"
archs=$(lipo -archs "$bundle/Contents/MacOS/pinto_slicer")
for arch in arm64 x86_64; do
    [[ " $archs " == *" $arch "* ]] || { echo "The bundle is missing the $arch slice: $archs" >&2; exit 1; }
done
echo "Universal executable contains: $archs"

# iconutil only accepts this exact set of names; anything else is rejected.
source_png="$root/assets/pinto-slicer.png"
iconset="$root/build/macos/icon.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
emit_icon() { sips -z "$1" "$1" "$source_png" --out "$iconset/$2" >/dev/null; }
emit_icon 16 icon_16x16.png
emit_icon 32 icon_16x16@2x.png
emit_icon 32 icon_32x32.png
emit_icon 64 icon_32x32@2x.png
emit_icon 128 icon_128x128.png
emit_icon 256 icon_128x128@2x.png
emit_icon 256 icon_256x256.png
emit_icon 512 icon_256x256@2x.png
emit_icon 512 icon_512x512.png
emit_icon 1024 icon_512x512@2x.png
iconutil -c icns "$iconset" -o "$bundle/Contents/Resources/pinto_slicer.icns"
rm -rf "$iconset"

cat > "$bundle/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>${app_name}</string>
    <key>CFBundleExecutable</key>
    <string>pinto_slicer</string>
    <key>CFBundleIconFile</key>
    <string>pinto_slicer.icns</string>
    <key>CFBundleIdentifier</key>
    <string>${app_id}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>${app_name}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${version}</string>
    <key>CFBundleVersion</key>
    <string>${version}</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.graphics-design</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>AGPL-3.0-only. See LICENSE and NOTICE.</string>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
EOF
plutil -lint "$bundle/Contents/Info.plist" >/dev/null
install -m644 "$root/LICENSE" "$root/NOTICE" "$bundle/Contents/Resources/"

# Ad-hoc signature: Apple Silicon refuses to run unsigned binaries, and a
# universal binary needs a signature that covers both slices. Sign last: any
# later change to the bundle contents invalidates it.
codesign --force --sign - --timestamp=none "$bundle"
codesign --verify --strict "$bundle"

dmg_stage="$root/build/macos/dmg"
rm -rf "$dmg_stage"
mkdir -p "$dmg_stage"
cp -R "$bundle" "$dmg_stage/$app_name.app"
ln -s /Applications "$dmg_stage/Applications"

dmg="$output_dir/Pinto-Slicer-${version}-universal.dmg"
mkdir -p "$output_dir"
rm -f "$dmg"
hdiutil create \
    -volname "$app_name" \
    -srcfolder "$dmg_stage" \
    -fs HFS+ \
    -format UDZO \
    -ov \
    "$dmg" >/dev/null

checksum=$(shasum -a 256 "$dmg" | awk '{print $1}')
printf '%s  %s\n' "$checksum" "$(basename -- "$dmg")" > "$output_dir/SHA256SUMS"
printf 'Created %s\nSHA256: %s\n' "$dmg" "$checksum"
