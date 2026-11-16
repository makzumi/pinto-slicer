# Release guide

Pinto Slicer currently publishes **Linux x86_64** packages through GitHub Releases:

- `Pinto-Slicer-VERSION-x86_64.AppImage`
- `Pinto-Slicer-VERSION-x86_64.flatpak`
- `SHA256SUMS`

The AppImage is built on Ubuntu 22.04 for a glibc 2.35 baseline. The Flatpak targets Freedesktop 26.08 and is compiled from source with offline Cargo dependencies. Both packages need a working Vulkan driver.

## Automated releases

The [Release workflow](.github/workflows/release.yml) runs for tags matching `v*` and can also be started manually from GitHub Actions. A manual run builds and tests the packages and uploads workflow artifacts without publishing a release.

For a tag build, the workflow:

1. Checks that the tag, Cargo package version, AppStream release, and screenshot URL agree.
2. Runs formatting, tests, Clippy, and metadata validation.
3. Builds the AppImage and checks its command-line entry point and a real Vulkan-rendered screenshot.
4. Builds and lints the Flatpak, installs the bundle, and runs CLI and GUI smoke tests.
5. Generates `SHA256SUMS` and publishes all assets to the tag's GitHub Release.

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
   git tag -a v0.1.0 -m 'Pinto Slicer 0.1.0'
   git push origin v0.1.0
   ```

Tags containing `alpha`, `beta`, or `rc` are published as GitHub pre-releases. Protect release tags in the repository settings to prevent accidental moves.

## Building packages locally

### AppImage

Build in an **Ubuntu 22.04** environment. The script deliberately rejects host binaries that require newer glibc symbols, missing libraries, a non-x86_64 ELF, or host-specific RPATHs.

```bash
cargo build --release --locked
packaging/build-appimage.sh
```

The script downloads a pinned `appimagetool`, verifies its SHA-256, stages `build/appimage/AppDir`, and writes the AppImage to `dist/`. It also runs an extracted-package CLI smoke test. Run the GUI smoke test on a machine with Vulkan, or use the release workflow's Xvfb/software-Vulkan test.

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
flatpak install --user ./dist/Pinto-Slicer-0.1.0-x86_64.flatpak
flatpak run io.github.makzumi.pinto-slicer --help
flatpak run io.github.makzumi.pinto-slicer --demo
```

Also test model opening/saving through portals, exports to removable storage, and Wayland and X11 sessions. The manifest intentionally has no network permission. Host filesystem access is needed for arbitrary CLI paths and atomic export writes beside the destination.

## Flathub and other platforms

Publishing a `.flatpak` bundle to GitHub does not list the app on Flathub. A hosted build requires a separate manifest review and an immutable source entry (`url`, `tag`, and full `commit`) instead of this checkout-oriented manifest's local `dir`/`file` sources. Keep `cargo-sources.json` beside the hosted manifest and check the current runtime and submission policies before proceeding. Flathub submission is not automated here.

macOS and Windows can later be added as separate build jobs in `release.yml`, with their artifacts downloaded into the same publish job.
