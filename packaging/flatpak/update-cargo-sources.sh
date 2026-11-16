#!/usr/bin/env bash
# Regenerate offline Flatpak sources after changing Cargo.lock.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
generator_commit=9d86d336a39b9af38b86f114b6ce5820fe260f91
work="$root/build/flatpak-generator"
venv="$work/venv"
generator="$work/flatpak-cargo-generator.py"
mkdir -p "$work"

if [[ ! -x $venv/bin/python ]]; then
    python3 -m venv "$venv"
fi
if ! "$venv/bin/python" -c 'import aiohttp, tomlkit' >/dev/null 2>&1; then
    "$venv/bin/pip" install --disable-pip-version-check \
        'aiohttp==3.14.5' 'tomlkit==0.15.2'
fi
if [[ ! -f $generator ]]; then
    curl -fL --retry 3 \
        "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/${generator_commit}/cargo/flatpak-cargo-generator.py" \
        -o "$generator"
fi

"$venv/bin/python" "$generator" "$root/Cargo.lock" \
    -o "$root/packaging/flatpak/cargo-sources.json"
printf 'Updated packaging/flatpak/cargo-sources.json\n'
