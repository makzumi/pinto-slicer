# Release guide

Pinto Slicer publishes packages for **Linux x86_64**, **macOS (universal)**, and **Windows x64** through GitHub Releases:

- `Pinto-Slicer-VERSION-x86_64.AppImage`
- `Pinto-Slicer-VERSION-x86_64.flatpak`
- `Pinto-Slicer-VERSION-universal.dmg`
- `Pinto-Slicer-VERSION-x64-portable.zip`
- `SHA256SUMS` (one file covering every asset)

The AppImage is built on Ubuntu 22.04 for a glibc 2.35 baseline. The Flatpak targets Freedesktop 26.08 and is compiled from source with offline Cargo dependencies. Both Linux packages need a working Vulkan driver. The DMG contains a lipo-joined Apple Silicon + Intel binary and is ad-hoc signed, not notarized. The Windows executable links the C runtime statically, so no redistributable is needed.

## Automated releases

The [Release workflow](.github/workflows/release.yml) runs for tags matching `v*` and can also be started manually from GitHub Actions. A manual run builds and tests the packages and uploads workflow artifacts without publishing a release.

For a tag build, the workflow:

1. Checks that the tag, Cargo package version, AppStream release, and screenshot URL agree.
2. Runs formatting, tests, Clippy, and metadata validation.
3. Builds the AppImage and checks its command-line entry point and a real Vulkan-rendered screenshot.
4. Builds and lints the Flatpak, installs the bundle, and runs CLI and GUI smoke tests.
5. Builds the universal macOS DMG, mounts it, verifies both architecture slices, the code signature, the bundle metadata, and the CLI.
6. Builds the Windows portable ZIP, extracts it, and runs the CLI checks.
7. Generates one `SHA256SUMS` covering all assets and publishes them to the tag's GitHub Release.

The workflow uses `GITHUB_TOKEN`; no long-lived release token is required. Only the final publish job has `contents: write` permission.

## Preparing a release

1. Update `version` in `Cargo.toml` and run `cargo check` so the root package version in `Cargo.lock` is updated too.
2. Add a version and date entry to `packaging/io.github.makzumi.pinto-slicer.metainfo.xml`. Update its release-details URL and ensure the screenshot URL contains the new tag.
3. Update `CHANGELOG.md`.
4. If dependencies changed in `Cargo.lock`, regenerate the offline Flatpak sources:

   ```bash
   packaging/flatpak/update-cargo-sources.sh
   ```

5. Run the local checks:

   ```bash
   cargo fmt --all -- --check
   cargo test --locked
   cargo clippy --locked --all-targets -- -D warnings
   desktop-file-validate packaging/io.github.makzumi.pinto-slicer.desktop
   appstreamcli validate --no-net packaging/io.github.makzumi.pinto-slicer.metainfo.xml
   ```

6. Commit every release input, including `LICENSE`, `NOTICE`, metadata, screenshot, packaging scripts, and generated Cargo sources. Push the release commit to `main`.
7. Recommended: run the **Release** workflow manually from that commit and inspect its artifacts before tagging.
8. Create and push an annotated tag whose name is exactly `v` plus the Cargo version:

   ```bash
   git tag -a v1.0.2 -m 'Pinto Slicer 1.0.2'
   git push origin v1.0.2
   ```

Tags containing `alpha`, `beta`, or `rc` are published as GitHub pre-releases. Protect release tags in the repository settings to prevent accidental moves.

## Building packages locally

### AppImage

Build in an **Ubuntu 22.04** environment. The script deliberately rejects host binaries that require newer glibc symbols, missing libraries, a non-x86_64 ELF, or host-specific RPATHs. It also needs `libxkbcommon0`, `libxkbcommon-x11-0`, and `libxcb-xkb1` installed, because those files are copied into the package.

```bash
cargo build --release --locked
packaging/build-appimage.sh
```

The script downloads a pinned `appimagetool`, verifies its SHA-256, stages `build/appimage/AppDir`, and writes the AppImage to `dist/`.

winit loads the keyboard stack with `dlopen()` instead of `DT_NEEDED`, so `ldd` never reports `libxkbcommon-x11`. Without bundling it, any system that does not ship that package panics at startup with:

```
panicked at xkbcommon-dl-0.4.2/src/x11.rs:59:28:
Library libxkbcommon-x11.so could not be loaded.
```

The script copies `libxkbcommon.so.0`, `libxkbcommon-x11.so.0`, and `libxcb-xkb.so.1` into `usr/lib` and fails if any is missing from the host or from the finished AppImage. `packaging/AppRun` puts that directory first on `LD_LIBRARY_PATH`, so the bundled copies win over the host and X11/xcb/Wayland/Vulkan still come from the system. It also runs an extracted-package CLI smoke test. Run the GUI smoke test on a machine with Vulkan, or use the release workflow's Xvfb/software-Vulkan test.

### Flatpak

Install Flatpak and either your distribution's current `flatpak-builder` or the Flatpak-maintained builder:

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub org.flatpak.Builder
```

Then build:

```bash
packaging/build-flatpak.sh
```

The manifest stages only source files, compiles with `cargo --offline --locked`, and creates an installable bundle in `dist/`. Build state and generated packages stay under the ignored `build/` and `dist/` directories.

To lint the resulting repository:

```bash
flatpak run --command=flatpak-builder-lint --filesystem=host org.flatpak.Builder repo build/flatpak-repo
```

Test the actual bundle before publishing:

```bash
flatpak install --user ./dist/Pinto-Slicer-1.0.2-x86_64.flatpak
flatpak run io.github.makzumi.pinto-slicer --help
flatpak run io.github.makzumi.pinto-slicer --demo
```

Also test model opening/saving through portals, exports to removable storage, and Wayland and X11 sessions. The manifest intentionally has no network permission. Host filesystem access is needed for arbitrary CLI paths and atomic export writes beside the destination.

## Flathub and other platforms

Publishing a `.flatpak` bundle to GitHub does not list the app on Flathub. A hosted build requires a separate manifest review and an immutable source entry (`url`, `tag`, and full `commit`) instead of this checkout-oriented manifest's local `dir`/`file` sources. Keep `cargo-sources.json` beside the hosted manifest and check the current runtime and submission policies before proceeding. Flathub submission is not automated here.

## macOS and Windows

Both builds run in the release workflow on GitHub-hosted runners, but they can be reproduced locally.

### macOS

Run on a Mac with the Xcode command line tools (`sips`, `iconutil`, `hdiutil`, `lipo`, `codesign`):

```bash
packaging/build-macos.sh
```

The script builds `aarch64-apple-darwin` and `x86_64-apple-darwin` (cross-compiling the Intel slice with the same SDK), strips both, joins them with `lipo`, generates the `.icns` with the exact `iconutil` icon-set names, writes `Info.plist`, ad-hoc signs the bundle, and writes `dist/Pinto-Slicer-VERSION-universal.dmg` plus `dist/SHA256SUMS`.

Signing is ad-hoc only. A distributable build that launches without a Gatekeeper prompt needs an Apple Developer ID certificate, `codesign --options runtime` with entitlements, and notarization (`notarytool submit --wait` followed by `stapler staple`). Add those steps here once a certificate is available; keep the ad-hoc signature otherwise so the Apple Silicon slice still runs.

### Windows

Run in PowerShell on Windows with the MSVC build tools and the `x86_64-pc-windows-msvc` target:

```powershell
pwsh packaging/build-windows.ps1
```

The script builds with `-C target-feature=+crt-static`, verifies `--help` runs, stages the executable with the icon, license, and readme, and writes `dist/Pinto-Slicer-VERSION-x64-portable.zip` plus `dist/SHA256SUMS`. Pass `-SkipBuild` to repackage an existing binary or `-Target aarch64-pc-windows-msvc` for Windows on Arm.
