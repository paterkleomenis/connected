# Flatpak Packaging for Connected

Manifests and metadata for the Flatpak packages.

## Files

- `io.github.paterkleomenis.Connected.yml` — the Flathub submission
  manifest. Builds the released git tag and sets `CONNECTED_FLATHUB_BUILD=1`
  so the app disables its in-app updater (Flathub delivers updates).
- `connected-desktop-bundle.yml` — the manifest for the self-hosted
  `.flatpak` bundles on GitHub releases (built by
  `.github/workflows/release.yml`, smoke-tested by `ci.yml`). Same as the
  Flathub one, except it keeps `--talk-name=org.freedesktop.Flatpak` for
  in-app self-updates and builds the working tree instead of a tag.
- `cargo-sources.json` — vendored crates, shared by both manifests.
- `io.github.paterkleomenis.Connected.{desktop,metainfo.xml,png}` — desktop
  entry, AppStream metainfo and 512×512 icon. These ship in the app source
  tree and get installed by the build (Flathub wants metadata upstream).

## How it works

- **Runtime**: `org.gnome.Platform//51` (ships the WebKitGTK the
  Dioxus/wry webview links against). GNOME 51 sits on Freedesktop 26.08,
  which is where the `rust-stable` toolchain extension comes from — no
  rustup, no downloads at build time.
- **Dependencies**: all crates (including the `single-instance` git dep)
  are vendored and checksum-pinned in `cargo-sources.json`, so
  `cargo build --offline --locked` runs without network.
- **Wayland/X11**: standard `--socket=wayland` + `--socket=x11`. The
  webview renders natively on Wayland; clipboard sync uses the X11
  clipboard as a fallback on compositors without the data-control protocol
  (Mutter), and data-control directly on KDE/wlroots.

## Regenerating cargo-sources.json

Whenever `Cargo.lock` changes:

```bash
pip install aiohttp tomlkit   # once
curl -O https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

## Building locally

```bash
flatpak install --user flathub \
    org.gnome.Sdk//51 org.gnome.Platform//51 \
    org.freedesktop.Sdk.Extension.rust-stable//26.08
flatpak install --user flathub org.flatpak.Builder
```

Bundle manifest from the working tree (same as `just build-flatpak`):

```bash
flatpak run org.flatpak.Builder --force-clean \
    build-dir packaging/flatpak/connected-desktop-bundle.yml
```

Flathub manifest from the released tag:

```bash
flatpak run org.flatpak.Builder --force-clean \
    --repo=target/flatpak-repo target/flatpak-build-dir \
    packaging/flatpak/io.github.paterkleomenis.Connected.yml
```

## Linting

```bash
flatpak run --command=flatpak-builder-lint org.flatpak.Builder \
    manifest packaging/flatpak/io.github.paterkleomenis.Connected.yml
flatpak run --command=flatpak-builder-lint org.flatpak.Builder \
    appstream packaging/flatpak/io.github.paterkleomenis.Connected.metainfo.xml
```

No `errors` allowed. Submitting a manifest whose comments read like a
prototype writeup also gets the PR closed without review — keep the
manifest terse.

## Submission

New apps go in as a PR against the `new-pr` branch of flathub/flathub —
see `FLATHUB_SUBMISSION.md` in this directory for the current state and
the review notes (permission justifications included in the PR body).
