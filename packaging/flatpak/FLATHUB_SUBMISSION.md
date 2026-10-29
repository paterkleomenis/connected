# Submitting Connected to Flathub

Notes for myself so I don't have to figure this out again next release.

## What goes in

The submission branch (`connected-submission` on my `flathub` fork) has two
files at the root:

- `io.github.paterkleomenis.Connected.yml` — the manifest. It builds the
  `3.5.4` tag and pins the commit next to the tag so Flathub's linter is
  happy and upstream re-tags can't silently change a published build.
- `cargo-sources.json` — the vendored crates. Everything (including the
  `single-instance` git dep) is checksum-pinned, so the buildbot needs
  zero network beyond pulling sources.

No desktop file / metainfo / icon in the submission — those ship in the app
source tree and get installed by the build, which is what Flathub wants
(metadata integrated upstream).

## How a submission works

Turns out there's no web form for this. New apps go in as a pull request
against the `new-pr` branch of flathub/flathub, and if it's approved the
reviewers merge it into a fresh repo under the Flathub org and give me
write access (2FA required, accept within a week).

The branch is already pushed. Remaining steps, in order:

1. Record a short video of the app doing an actual file transfer between
   two devices. Reviewers want to see the thing running, can't skip this.
2. Read `PR_BODY.md` (next to this file), fix it up so every line is
   something I actually mean, attach the video, then open the PR against
   `new-pr` (NOT master):

   ```bash
   gh pr create --repo flathub/flathub --base new-pr \
       --head paterkleomenis:connected-submission \
       --title "Add io.github.paterkleomenis.Connected" \
       --body-file target/flathub-submission/PR_BODY.md
   ```

3. When reviewers ask for a test build, comment `bot, build` on the PR.
4. Answer reviewer comments myself.

Note: the video is recorded (path in `PR_BODY.md`, 2.8 MB so it embeds
inline). After opening the PR, edit the body, drag the video in, and tick
the video item + the last AI checkbox.

## When they ask about permissions

`--filesystem=host` is the one that will get questions. The honest answer:
this is a file transfer app, phones and other computers browse the folders
I shared through it and files I receive get saved wherever I pick — same
reason a file manager wants host access. Transfers are p2p over QUIC,
encrypted, no server in the middle.

Both `--socket=wayland` and `--socket=x11`: the webview renders through
the wayland socket; clipboard sync needs the X11 clipboard on compositors
without the wayland data-control protocol (GNOME/Mutter), which
`--socket=wayland` alone can't cover. See `ClipboardCache` in
`desktop/src/utils.rs`.

`--share=network`: mDNS discovery + the transfers themselves, all LAN
side. The only internet touch is checking GitHub for release notes.

There's deliberately no `org.freedesktop.Flatpak` talk name: flathub apps
aren't supposed to install their own updates, so the manifest sets
`CONNECTED_FLATHUB_BUILD=1` and the app leaves updating to Flathub. The
GitHub-release bundles use a separate manifest
(`connected-desktop-bundle.yml`) that keeps the permission so those
installs can still self-update.

## Review log

- 2026-10-25: opened as #10830, auto-closed by the checker (video
  checklist not met), fixed and reopened by a reviewer.
- 2026-10-25: requested changes — drop the prototype-style comments from
  the manifest, move to the GNOME 51 runtime, replace the
  wayland-socket-mount workaround with standard sockets and no wrapper.
  Done in the `new-pr`-targeted branch; awaiting re-review.

## Next releases

`just prepare-release X.Y.Z`, bump `tag:` (+ pinned commit) in the
manifest, regen `cargo-sources.json` if the lock moved, push, update the
flathub repo. `bump-version.sh` doesn't touch the manifest tag, that part
is manual.
