# Flatpak Packaging for Connected

This directory contains the files necessary to build a Flatpak package for Connected.

## Prerequisites

- `flatpak`
- `flatpak-builder`

You also need the GNOME 48 SDK and Platform runtimes, along with the Rust extension:

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub org.gnome.Sdk//48 org.gnome.Platform//48 org.freedesktop.Sdk.Extension.rust-stable//24.08
```

## Building

To build the Flatpak package, run the following command from the root of the repository:

```bash
flatpak-builder --force-clean build-dir packaging/flatpak/com.paterkleomenis.Connected.yml
```

## Running

Once built, you can run the application with:

```bash
flatpak-builder --run build-dir packaging/flatpak/com.paterkleomenis.Connected.yml connected-desktop
```

## Installing

To install the built package locally:

```bash
flatpak-builder --user --install --force-clean build-dir packaging/flatpak/com.paterkleomenis.Connected.yml
```

## Note on Dependencies

This manifest currently uses a simple build system that expects network access to download Rust crates during the build. For a production-ready Flathub submission, dependencies should be vendored or a `generated-sources.json` should be used.
