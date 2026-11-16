#!/usr/bin/env bash
# Build an x86_64 AppImage from an existing Linux release binary.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
export LC_ALL=C
export TZ=UTC
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$root" show -s --format=%ct HEAD 2>/dev/null || date +%s)}
app_id=io.github.makzumi.pinto-slicer
binary=${1:-target/release/pinto_slicer}
version=$(awk -F '"' '/^version = / { print $2; exit }' "$root/Cargo.toml")
output=${2:-dist/Pinto-Slicer-${version}-x86_64.AppImage}

[[ $binary = /* ]] || binary="$root/$binary"
[[ $output = /* ]] || output="$root/$output"
[[ -f $binary ]] || { echo "Missing release binary: $binary" >&2; exit 1; }
[[ $(uname -m) == x86_64 ]] || { echo "This script currently packages x86_64 only." >&2; exit 1; }
for tool in readelf objdump ldd strip; do
    command -v "$tool" >/dev/null 2>&1 || { echo "Required tool is missing: $tool" >&2; exit 1; }
done
readelf -h "$binary" | grep -q 'Machine:.*Advanced Micro Devices X86-64' || {
    echo "The input is not an x86_64 ELF binary: $binary" >&2
    exit 1
}
if ldd "$binary" | grep -q 'not found'; then
    ldd "$binary" >&2
    echo "The release binary has missing shared libraries." >&2
    exit 1
fi
if readelf -d "$binary" | grep -E '\((RPATH|RUNPATH)\)'; then
    echo "Remove host-specific RPATH/RUNPATH entries before packaging." >&2
    exit 1
fi
highest_glibc=$(objdump -T "$binary" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -Vu | tail -1)
if [[ -z $highest_glibc || $(printf '2.35\n%s\n' "$highest_glibc" | sort -V | tail -1) != 2.35 ]]; then
    echo "The binary requires glibc $highest_glibc; AppImages must support Ubuntu 22.04 (glibc 2.35)." >&2
    echo "Build it in the release workflow or an Ubuntu 22.04 container." >&2
    exit 1
fi

# Pin and verify the packaging tool before executing it.
tool_version=1.9.1
tool_sha256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
tool_root="$root/build/appimage-tool"
tool_image="$tool_root/appimagetool-x86_64.AppImage"
tool_dir="$tool_root/appimagetool"
runtime="$tool_root/runtime-x86_64"
runtime_sha256=617aa77314d57c8b8e4fae9c3f0b35af7b30d5529cb69e98f64f92510ed98a46
mkdir -p "$tool_root"
if [[ ! -f $tool_image ]]; then
    curl -fL --retry 3 \
        "https://github.com/AppImage/appimagetool/releases/download/${tool_version}/appimagetool-x86_64.AppImage" \
        -o "$tool_image.tmp"
    echo "$tool_sha256  $tool_image.tmp" | sha256sum -c -
    mv "$tool_image.tmp" "$tool_image"
fi
echo "$tool_sha256  $tool_image" | sha256sum -c -
chmod +x "$tool_image"
if [[ ! -x $tool_dir/AppRun ]]; then
    rm -rf "$tool_dir" "$tool_root/squashfs-root"
    (cd "$tool_root" && "$tool_image" --appimage-extract >/dev/null)
    mv "$tool_root/squashfs-root" "$tool_dir"
fi
if [[ ! -f $runtime ]]; then
    curl -fL --retry 3 \
        https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64 \
        -o "$runtime.tmp"
    echo "$runtime_sha256  $runtime.tmp" | sha256sum -c -
    mv "$runtime.tmp" "$runtime"
fi
echo "$runtime_sha256  $runtime" | sha256sum -c -

appdir="$root/build/appimage/AppDir"
rm -rf "$appdir"
mkdir -p \
    "$appdir/usr/bin" \
    "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/512x512/apps" \
    "$appdir/usr/share/metainfo" \
    "$appdir/usr/share/doc/pinto-slicer" \
    "$(dirname -- "$output")"

install -m755 "$binary" "$appdir/usr/bin/pinto_slicer"
strip --strip-unneeded "$appdir/usr/bin/pinto_slicer"
install -m755 "$root/packaging/AppRun" "$appdir/AppRun"
install -m644 "$root/packaging/$app_id.desktop" "$appdir/$app_id.desktop"
install -m644 "$root/packaging/$app_id.desktop" "$appdir/usr/share/applications/$app_id.desktop"
install -m644 "$root/assets/pinto-slicer.png" "$appdir/$app_id.png"
install -m644 "$root/assets/pinto-slicer.png" "$appdir/.DirIcon"
install -m644 "$root/assets/pinto-slicer.png" "$appdir/usr/share/icons/hicolor/512x512/apps/$app_id.png"
install -m644 "$root/packaging/$app_id.metainfo.xml" "$appdir/usr/share/metainfo/$app_id.appdata.xml"
install -m644 "$root/LICENSE" "$root/NOTICE" "$root/README.md" "$appdir/usr/share/doc/pinto-slicer/"

if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$appdir/$app_id.desktop"
fi

rm -f "$output"
find "$appdir" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
# CI validates AppStream offline. AppImageTool's online validation would reject
# the immutable screenshot URL before a first release tag has been published.
ARCH=x86_64 "$tool_dir/AppRun" --no-appstream --runtime-file "$runtime" "$appdir" "$output"
chmod +x "$output"

# Exercise the packaged entry point without requiring FUSE or a display.
smoke_dir="$root/build/appimage-smoke"
rm -rf "$smoke_dir"
mkdir -p "$smoke_dir"
(
    cd "$smoke_dir"
    "$output" --appimage-extract >/dev/null
    squashfs-root/AppRun --help | grep -q '^Pinto Slicer'
)

printf 'Created %s\n' "$output"
