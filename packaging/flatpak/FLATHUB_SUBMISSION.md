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

Submissions are pull requests against the `new-pr` base branch of
[flathub/flathub](https://github.com/flathub/flathub) (approved submissions
are then merged into a new repo under the Flathub org).

The mechanical part is already done:

- fork: <https://github.com/paterkleomenis/flathub>
- branch `connected-submission` contains, at the repo root,
  `io.github.paterkleomenis.Connected.yml` (with pinned commit + AI
  disclosure header) and `cargo-sources.json`, committed as
  "Add io.github.paterkleomenis.Connected"
- a filled-in PR-template draft is at
  `target/flathub-submission/PR_BODY.md`

The PR itself is opened manually by the maintainer (Flathub's generative-AI
policy requires it not be opened by an AI agent):

1. Record a short video of Connected running (as the Flatpak) doing a file
   transfer between two devices — reviewers want to see it working.
2. Open the PR **against the `new-pr` base branch** with the title
   `Add io.github.paterkleomenis.Connected`, either via the web UI
   (<https://github.com/flathub/flathub/compare/new-pr...paterkleomenis:connected-submission>)
   or with:

   ```bash
   gh pr create --repo flathub/flathub --base new-pr \
       --head paterkleomenis:connected-submission \
       --title "Add io.github.paterkleomenis.Connected" \
       --body-file target/flathub-submission/PR_BODY.md
   ```

   Edit `PR_BODY.md` first: tick the video item once the video is attached,
   and adjust the wording so every statement is yours.
3. Reply to reviewer comments yourself (not via AI tools), and comment
   `bot, build` to trigger a test build when asked.

## Regenerating the submission files

```bash
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
mkdir -p target/flathub-submission
cp packaging/flatpak/cargo-sources.json target/flathub-submission/
sed "s|^        tag: 3.5.4$|        tag: <NEW TAG>\n        commit: $(git rev-parse <NEW TAG>)|" \
    packaging/flatpak/io.github.paterkleomenis.Connected.yml \
    > target/flathub-submission/io.github.paterkleomenis.Connected.yml
```

## Generative-AI disclosure (paste into the submission PR)

Per [Flathub's Generative AI policy](https://docs.flathub.org/docs/for-app-authors/requirements#generative-ai-policy),
include this in the submission PR description:

> **AI assistance disclosure**
>
> The Flatpak packaging for this submission was prepared with the assistance
> of an AI coding agent (an interactive coding session run by the
> maintainer), covering approximately the following extent:
>
> - the Flatpak manifest (`io.github.paterkleomenis.Connected.yml`) and the
>   self-hosted bundle variant,
> - the vendored dependency sources (`cargo-sources.json`, generated with
>   the official `flatpak-cargo-generator` tool),
> - the Wayland/clipboard launch wrapper comments, and
> - the submission documentation in `packaging/flatpak/`.
>
> All of it was reviewed, tested and approved by the maintainer before
> submission. The application source code itself (Rust core, desktop UI,
> Android/iOS apps) was written by the maintainer. No AI tools were used to
> open this pull request or to interact with the review.

> [!IMPORTANT]
> Flathub's policy forbids AI agents from opening or automating submission
> pull requests, and AI-generated review interactions can get the
> submission rejected or the account banned. Submit through the website
> yourself, and do **not** ask AI tools to reply to reviewers on the
> submission PR — handle review comments manually.

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
