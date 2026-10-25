# Flatpak Packaging for Connected

This directory contains the Flatpak manifests and metadata for Connected.

## Files

- `io.github.paterkleomenis.Connected.yml` — **the Flathub submission
  manifest**. Builds the released git tag, uses only the permissions Flathub
  allows, and marks the build with `CONNECTED_FLATHUB_BUILD=1` so the app
  disables its in-app updater (Flathub delivers updates instead).
- `connected-desktop-bundle.yml` — the manifest for the self-hosted
  `.flatpak` bundles published on GitHub releases (built by
  `.github/workflows/release.yml`, smoke-tested by `ci.yml`). Identical to
  the Flathub manifest except it keeps `--talk-name=org.freedesktop.Flatpak`
  so the in-app updater can install the next release bundle via
  `flatpak-spawn --host`, and it builds the working tree (`type: dir`)
  instead of a tag.
- `cargo-sources.json` — generated vendored-crate sources (see below),
  shared by both manifests
- `io.github.paterkleomenis.Connected.desktop` — desktop entry (installed
  from the app source tree)
- `io.github.paterkleomenis.Connected.metainfo.xml` — AppStream metainfo
- `io.github.paterkleomenis.Connected.png` — 512×512 icon
- `connected-desktop-wayland-socket` — launch wrapper that picks up the
  host's Wayland socket (see the manifest comments and commit `2d3d6b8`)

## How the builds work

- **Runtime**: `org.gnome.Platform//50` (bundles the WebKitGTK required by
  the Dioxus/wry webview). It is based on Freedesktop 25.08, which is where
  the `org.freedesktop.Sdk.Extension.rust-stable` toolchain is resolved
  from — no rustup, no toolchain or crate downloads happen at build time.
- **Dependencies**: every crate (and the `single-instance` git dependency)
  is vendored in `cargo-sources.json` and verified by checksum, so
  `cargo build --offline --locked` needs no network.
- **Metadata**: the desktop file, metainfo and icon are installed from the
  app source tree — Flathub requires app metadata to be integrated
  upstream, not copied into the submission.

## Regenerating cargo-sources.json

Whenever `Cargo.lock` changes, regenerate the vendored sources with
[flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools):

```bash
pip install aiohttp tomlkit   # once
curl -O https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

## Building locally

Prerequisites: `flatpak` and the [Flatpak Builder app]
(https://flathub.org/apps/org.flatpak.Builder), plus the runtimes:

```bash
flatpak install --user flathub \
    org.gnome.Sdk//50 org.gnome.Platform//50 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak install --user flathub org.flatpak.Builder
```

Then build the bundle manifest from the working tree (same as
`just build-flatpak`):

```bash
flatpak run org.flatpak.Builder --force-clean \
    build-dir packaging/flatpak/connected-desktop-bundle.yml
```

or build the Flathub manifest from the released tag:

```bash
flatpak run org.flatpak.Builder --force-clean \
    --repo=target/flatpak-repo target/flatpak-build-dir \
    packaging/flatpak/io.github.paterkleomenis.Connected.yml
```

To install and run the exported repo build:

```bash
flatpak install --user --bundle target/flatpak-repo/io.github.paterkleomenis.Connected.flatpak
flatpak run io.github.paterkleomenis.Connected
```

## Linting (Flathub's automated checks)

```bash
flatpak run --command=flatpak-builder-lint org.flatpak.Builder \
    manifest packaging/flatpak/io.github.paterkleomenis.Connected.yml
flatpak run --command=flatpak-builder-lint org.flatpak.Builder \
    appstream packaging/flatpak/io.github.paterkleomenis.Connected.metainfo.xml
```

Both must report no `errors` before submission. (`--nosocket=wayland` is
*never* accepted by Flathub's linter, which is why the manifests don't
grant the wayland socket at all and instead mount the host's sockets via
`--filesystem=xdg-run/wayland-*` + the wrapper script.)

## Submitting to Flathub — checklist

1. **Commit the app-id rename and the Flatpak-specific code first.** The
   staged changes (the `io.github.paterkleomenis.*` file names, the
   Background-portal autostart, and `is_flathub_build()`/`CONNECTED_FLATHUB_BUILD`
   in `core/src/update.rs`) must be part of the tag the manifest builds —
   a build of the old `3.5.2` tag would still offer in-app self-updates,
   which Flathub apps must not do.
2. **(Re)create the `3.5.2` tag** (or cut a new release; then update `tag:`
   in `io.github.paterkleomenis.Connected.yml` — `scripts/bump-version.sh`
   does *not* do this — and add the `<releases>` entry via
   `just prepare-release`), and push the tag.
3. **Pin the resolved commit** next to `tag:` in the manifest — Flathub's
   linter expects `tag` + `commit`, and pinning protects against upstream
   re-tagging. Get it via `git rev-parse 3.5.2`.
4. **Regenerate `cargo-sources.json`** if `Cargo.lock` changed since.
5. Run the linters above and fix any errors.
6. Submit via <https://flathub.org> (new apps are submitted through the
   website; upload the manifest and `cargo-sources.json` together).

### Permission justifications (for the submission review)

- `--filesystem=host` — the app is a file-transfer tool: remote peers browse
  the sender's real filesystem, and received files are saved at
  user-chosen locations. This is the same access a file manager needs.
- `--socket=x11` (not `fallback-x11`) — Mutter does not implement the
  Wayland data-control protocol, so clipboard sync falls back to the X11
  clipboard even on Wayland hosts; `fallback-x11` would not provide the
  socket there (see `ClipboardCache` in `desktop/src/utils.rs`).
- `--filesystem=xdg-run/wayland-*` — direct access to the host's compositor
  socket for the `wlroots`/KDE data-control clipboard (GNOME hosts use the
  X11 fallback above); see commit `2d3d6b8`.
- `--share=network` — mDNS discovery and QUIC transfers on the LAN; the app
  is serverless and works peer-to-peer.
- No `--talk-name=org.freedesktop.Flatpak` in the Flathub manifest on
  purpose: Flathub apps must not self-install updates. The manifest sets
  `CONNECTED_FLATHUB_BUILD=1` so the app disables its in-app updater and
  lets Flathub deliver updates. (The GitHub-release bundle manifest keeps
  the permission so those bundles can self-update.)
