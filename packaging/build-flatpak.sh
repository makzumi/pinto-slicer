#!/usr/bin/env bash
# Build an installable Flatpak bundle from this checkout.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
app_id=io.github.makzumi.pinto-slicer
manifest="$root/packaging/flatpak/$app_id.yml"
version=$(awk -F '"' '/^version = / { print $2; exit }' "$root/Cargo.toml")
runtime_version=$(awk '/^runtime-version:/ { gsub(/["\047]/, "", $2); print $2; exit }' "$manifest")
arch=$(flatpak --default-arch)
output=${1:-dist/Pinto-Slicer-${version}-${arch}.flatpak}
[[ $output = /* ]] || output="$root/$output"

if command -v flatpak-builder >/dev/null 2>&1; then
    builder=(flatpak-builder)
elif flatpak info org.flatpak.Builder >/dev/null 2>&1; then
    builder=(flatpak run --command=flatpak-builder --filesystem=host org.flatpak.Builder)
else
    cat >&2 <<'EOF'
flatpak-builder is required. Install it with your distribution, or run:
  flatpak install --user flathub org.flatpak.Builder
EOF
    exit 1
fi

flatpak remote-add --user --if-not-exists flathub \
    https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user -y --noninteractive flathub \
    "org.freedesktop.Platform//$runtime_version" \
    "org.freedesktop.Sdk//$runtime_version" \
    "org.freedesktop.Sdk.Extension.rust-stable//$runtime_version"

build_dir="$root/build/flatpak-app"
repo="$root/build/flatpak-repo"
state_dir="$root/build/flatpak-state"
mkdir -p "$repo" "$state_dir" "$(dirname -- "$output")"

cd "$root"
"${builder[@]}" \
    --user \
    --force-clean \
    --disable-rofiles-fuse \
    --state-dir="$state_dir" \
    --repo="$repo" \
    "$build_dir" \
    "$manifest"

# Check the built executable before creating the downloadable bundle. The
# release workflow additionally installs and runs the actual bundle.
test -x "$build_dir/files/bin/pinto_slicer"
flatpak build --with-appdir "$build_dir" \
    /app/bin/pinto_slicer --help | grep -q '^Pinto Slicer'

rm -f "$output"
flatpak build-bundle \
    --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    "$repo" \
    "$output" \
    "$app_id" \
    stable

printf 'Created %s\n' "$output"
printf 'Install with: flatpak install --user %s\n' "$output"
