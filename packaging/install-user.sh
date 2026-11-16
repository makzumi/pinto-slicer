#!/bin/sh
# Install Pinto Slicer for the current user, including reliable KDE/Wayland
# taskbar branding. No root access or system-wide files are needed.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
app_id="io.github.makzumi.pinto-slicer"
data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}
bin_dir="$HOME/.local/bin"
icon="$data_home/icons/hicolor/512x512/apps/$app_id.png"
desktop="$data_home/applications/$app_id.desktop"

cargo build --release --locked --manifest-path "$root/Cargo.toml"
install -Dm755 "$root/target/release/pinto_slicer" "$bin_dir/pinto_slicer"
install -Dm644 "$root/assets/pinto-slicer.png" "$icon"
install -Dm644 "$root/packaging/$app_id.desktop" "$desktop"

# Plasma can retain stale theme-cache entries across icon replacements. An
# absolute path is valid in a desktop entry and makes the running window's
# launcher resolve this exact artwork instead of an old themed icon.
escaped_icon=$(printf '%s\n' "$icon" | sed 's/[&|]/\\&/g')
sed -i "s|^Icon=.*|Icon=$escaped_icon|" "$desktop"

# Remove superseded launchers and icons from pre-release installations.
rm -f "$data_home/applications/pinto-slicer.desktop"
rm -f "$data_home/icons/hicolor/512x512/apps/pinto-slicer.png"
rm -f "$data_home/icons/hicolor/256x256/apps/pinto-slicer.png"
rm -f "$data_home/icons/hicolor/scalable/apps/pinto-slicer.svg"

update-desktop-database "$data_home/applications"
if command -v kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 --noincremental
fi

printf 'Installed Pinto Slicer in %s\n' "$bin_dir/pinto_slicer"
printf 'Installed Wayland launcher in %s\n' "$desktop"
