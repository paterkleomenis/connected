# Flathub Submission Guide

Everything needed to submit Connected to Flathub is in
`target/flathub-submission/` (git-ignored build output — regenerate when
needed, see below).

## Files to upload

| File | Purpose |
|---|---|
| `target/flathub-submission/io.github.paterkleomenis.Connected.yml` | The manifest (submission copy — has `tag` + pinned `commit`) |
| `target/flathub-submission/cargo-sources.json` | Vendored crates referenced by the manifest |

Nothing else is needed: the desktop file, AppStream metainfo and icon come
from the `3.5.4` source tree during the Flathub build (a Flathub requirement
— metadata must be integrated upstream). The launch wrapper is also in the
source tree.

## Submission steps

1. Log in at <https://flathub.org> **with the GitHub account that owns the
   repo** (`paterkleomenis`) — for an `io.github.<owner>.<app>` app id this
   also makes GitHub verification straightforward afterwards.
2. Submit a new app: enter the app id **`io.github.paterkleomenis.Connected`**
   and upload the two files above.
3. In the permission-review form, paste the justification text below.
4. Flathub's buildbot builds the tag automatically; the submission PR is
   reviewed by Flathub maintainers. Respond to review comments in the PR.

## Regenerating the submission files

```bash
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
mkdir -p target/flathub-submission
cp packaging/flatpak/cargo-sources.json target/flathub-submission/
sed "s|^        tag: 3.5.4$|        tag: <NEW TAG>\n        commit: $(git rev-parse <NEW TAG>)|" \
    packaging/flatpak/io.github.paterkleomenis.Connected.yml \
    > target/flathub-submission/io.github.paterkleomenis.Connected.yml
```

## Permission justifications (paste-ready)

> **`--filesystem=host`**
>
> Connected is a peer-to-peer file-transfer tool (an open alternative to
> AirDrop/KDE Connect). Two features need full filesystem access:
>
> 1. **Remote file browsing**: a linked phone or computer browses the files
>    this machine shares through Connected. The shared tree is the user's
>    real filesystem — the same access any file manager needs. The user
>    picks which folders are shared in Connected's settings; peers only see
>    what they are allowed to browse.
> 2. **Saving received files anywhere**: transfers are saved at
>    user-chosen locations (Downloads, Documents, USB drives under
>    /run/media, ...), which the portal file dialogs cannot express for
>    background/automated transfers.
>
> All transfers are end-to-end encrypted (QUIC + TLS) and go only to the
> user's own paired devices. No data is sent to any server.

> **`--socket=x11` (instead of `fallback-x11`)**
>
> Clipboard synchronization falls back to the X11 clipboard when the
> compositor does not implement the Wayland data-control protocol — Mutter
> (GNOME) does not. On a Wayland host session `--socket=fallback-x11`
> provides no X11 socket at all, so the clipboard sync would silently fail
> on GNOME, the primary target. See `ClipboardCache` in
> `desktop/src/utils.rs`.

> **`--filesystem=xdg-run/wayland-0..32,99`**
>
> Direct, unproxied access to the host's Wayland sockets for the
> wlroots/KDE data-control clipboard path (GNOME uses the X11 fallback
> above). A wrapper script exports `WAYLAND_DISPLAY` to the first mounted
> socket. Sockets 0–32 cover every socket `wl_display_add_socket_auto`
> can return; wayland-99 is Mir's. Only Unix sockets are reachable through
> these grants.

> **`--share=network`**
>
> mDNS discovery of nearby devices and the QUIC data transfers themselves
> run on the local network. The app is serverless: transfers are
> peer-to-peer between paired devices only. The same grant also lets the
> app ask GitHub's public API for release notes (no downloads are
> installed; Flathub remains the update channel for this build).

> **No `--talk-name=org.freedesktop.Flatpak`**
>
> Deliberately omitted: Flathub applications must not request host-spawn
> access to self-install updates. The manifest sets
> `CONNECTED_FLATHUB_BUILD=1`, which makes the app treat Flathub as its
> update store (its own updater is disabled in code).

## After publication

For every later release: `just prepare-release X.Y.Z`, set `tag:` (+ pinned
`commit:`) in `io.github.paterkleomenis.Connected.yml`, regenerate
`cargo-sources.json` if the lock changed, push the tag, and update the
`flathub/io.github.paterkleomenis.Connected` repository (Flathub's
external-data-checker opens PRs for tag updates — review and regenerate the
cargo sources as part of merging them).
