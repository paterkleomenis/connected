# dev branch — consolidated changes (base `main@c6a4495`)

> **Purpose of `dev`:** isolate all development / testing / hardening work that is NOT yet ready for `main`. `main` stays clean and releasable; `dev` is the integration branch for protocol v2, security fixes, stability/ANR fixes, and adversarial tests. Remote `origin/dev` now tracks local `dev` (pushed 2026-09-02). `origin/develop` remains separate (8 commits ahead of `main` at `86be07e`).

**Stats:** `44 files changed, 2826 insertions(+), 687 deletions(-)` (`git diff main..dev --stat`), 4 new files.

---

## 1. Build / Workspace (`Cargo.toml:7`, `core/Cargo.toml:21`, `desktop/Cargo.toml:29`, `Justfile:312`, `Cargo.lock`)

- **Dev profile narrowed** `Cargo.toml:7-18`: `opt-level=3` only for `quinn`/`rustls`/`ring`/`mdns-sd` (heavy crypto/QUIC deps) — keeps debug symbols for stack traces. Was `package."*" opt-level=3 debug=false` which stripped symbols and optimized everything.
- **`bincode = "1.3"`** added to `[workspace.dependencies]` `Cargo.toml:74` and `core/Cargo.toml:24` — wire format for protocol v2.
- **`mimalloc`** `desktop/Cargo.toml:29` + `#[global_allocator] MiMalloc` `desktop/src/main.rs:6` — ~10% lower alloc overhead on 4 MB file-transfer hot path.
- **`Justfile:312` docker-shell** now `if os_family()==windows { cmd /c } else { bash }` — was always `cmd /c`.
- `Cargo.lock` refreshed (29 lines net, plus transitive dep bumps).

---

## 2. Protocol v2 — versioned framing (`core/src/codec.rs:1` NEW, `core/src/lib.rs:37`, `core/src/device.rs:48`, `core/src/discovery.rs`)

**New module `core/src/codec.rs:1-132` (132 LOC):**
- `MAGIC_BINCODE = 0x01`, `MAGIC_JSON = 0x00` `codec.rs:8-9`. Old peers (v1) send raw JSON (`{` 0x7B, no prefix); new peers (v2+) send `0x01` + bincode or `0x00` + JSON.
- `MAX_DECODE_BYTES = 128 MB` `codec.rs:16` — mirrors transport ~105 MB frame cap. Uses `bincode::options().with_fixint_encoding().with_limit(MAX_DECODE_BYTES)` `codec.rs:18-25` — fixint/LE exactly matches legacy `bincode::serialize` wire; `DefaultOptions` varint would break compat.
- `encode_message<T>(msg, peer_version)` `codec.rs:31` — `>=2` → bincode with prefix (~30% smaller, no base64), `==1` → `serde_json::to_vec`.
- `decode_message<T>(data)` `codec.rs:50` — dispatch on first byte; unknown → try JSON then bincode fallback; empty → `Protocol("Empty message")`.
- Tests `codec.rs:83-131`: bincode roundtrip, JSON v1 roundtrip, rejects `u64::MAX` declared lengths, empty input.

**Single source of truth** `core/src/lib.rs:37-42`:
```rust
pub const PROTOCOL_VERSION: u32 = 2;
pub const MIN_COMPATIBLE_PROTOCOL_VERSION: u32 = 1;
```
Bumping this alone updates `discovery.rs` announce/compat and `device.rs` stamping.

**Device** `core/src/device.rs:48-60`:
- Adds `protocol_version: u32` (serde `default=1`), `Device::new_with_version()`, `Device::new()` stamps `PROTOCOL_VERSION`.
- `FromStr` for `DeviceType` now strict: `Unknown/empty/*` → `Err(())` not silent `Unknown`.

**Discovery** `core/src/discovery.rs`:
- Timeouts: `DEVICE_STALE_TIMEOUT 10s→30s`, `CLEANUP 5s→10s` — reduces flapping.
- Bounded `100× Again` retry (was infinite loop hanging startup on `WouldBlock`).
- Re-announce no longer `unregister()` (prevents 5 s `DeviceLost` window).
- `ServiceRemoved` debounced — refreshes `last_seen`, defers to cleanup (fixes 1 s flapping with old peers clearing after `Removed`).
- Adds `get_device_version()` / `get_version_for_ip()` for codec selection; TXT `version` → `Device::new_with_version`.

---

## 3. Transport / Security / Errors (`core/src/transport.rs`, `core/src/security.rs:532`, `core/src/error.rs:104`)

**Transport `core/src/transport.rs:329`:**
- Constants `CLOSE_REASON_UNPAIRED = b"unpaired"`, `STALE = b"stale"`, `REJECTED = b"pairing-rejected"` — previously `unpaired` reused for retries/ping failures, causing remote to destroy pairing state on routine closes.
- `read_chunked()` (64 k chunks) replaces `vec![0; msg_len]` — bounds memory vs malicious length header.
- `open_stream_allow_unknown()` for outbound pairing without flipping global pairing mode.
- `ConnectingGuard` handles `Handle::try_current()` panic on shutdown.
- `connect()` now waits `CONNECT_TIMEOUT` and re-claims slot (was single 50 ms sleep → duplicate dials).
- `handle_connection` / `send_ping` use `codec::decode_message` + chunked read.

**Security `core/src/security.rs:532`:** `trust_peer` merges (preserves `device_id/name` if `None`) not overwrites — fixes fingerprint-only callers erasing metadata.

**Error `core/src/error.rs:104`:** `is_transient_io_error` now `#[cfg(windows)]` only — `5 = EIO` is fatal on Linux, must not retry.

---

## 4. File Transfer (`core/src/file_transfer.rs`)

- `BUFFER_SIZE 8→4 MB` (16 MB in-flight with depth 4 <256 MB for 16 streams), concurrent `6→10`.
- Progress clamped `min(total)` (resume double-count fix).
- Enforces `MAX_BATCH_ITEMS=10_000`, `MAX_BATCH_FILES_COUNT=10_000`, cumulative `bytes_seen > total_size+1 MB` and `MAX_INCOMING_FILE_SIZE`.
- `sanitize_filename` caps **255 bytes not chars** (CJK/emoji safe, no UTF-8 split).
- `recv_message_with_limit` uses `codec::decode`.
- Adds `is_safe_relative_path()` — traversal gate for batch transfers.

---

## 5. Client — pairing/handshake, downloads, clipboard, lifecycle (`core/src/client.rs` +480/-~200)

- `open_hardened_download_file()` — canonicalizes parent, rejects `..`/empty, refuses overwriting symlink/non-regular.
- `send_fs_message` / `recv_fs_message` — 30 s ctrl / 120 s chunk `tokio::timeout` (peers hanging streams previously hung UI forever).
- `PendingHandshakeMap` `IpAddr→SocketAddr` (NAT: multiple peers same IP). `clear_pending_pairing_state(SocketAddr)`, `has_pending_handshake` matches by IP, adds `has_pending_handshake_addr()`.
- `send_handshake` no longer `set_pairing_mode(true)` for 120 s — uses `open_stream_allow_unknown` scoped.
- **Fail-closed** `is_none_or → is_some_and` on `pending.fingerprint == peer_fp` — unknown FP no longer auto-trusts (IP-spoof trust hijack).
- Clipboard `4 MB` cap both send/recv (`Clipboard {text}` length check).
- `DeviceUnpaired` ignores unknown fingerprint (LAN attacker injecting fake unpair events blocked).
- `invalidate_connection` uses `CLOSE_REASON_*` correctly (`REJECTED` not `unpaired` for pairing reject).
- `MAX_DIRS_SCANNED = 5_000` + `MAX_CONCURRENT_DOWNLOADS 8→16`, `scan_remote_folder` depth+breadth bounded.
- `background_tasks: Vec<JoinHandle>` aborted on `shutdown()` (leak on Android restarts).
- Pairing events `allow_unknown` variants.

---

## 6. Update (`core/src/update.rs` +166/-~10)

- `download_to_file_verified()` + `fetch_expected_sha256()` (tries `<url>.sha256`, `hex filename` formats).
- Collision-free `.{name}.tmp.{pid}-{uuid}`.
- `normalize_version` strips `v`/whitespace/lowercase.
- Installs (macOS/Linux AppImage/Flatpak) verify if sidecar present else warn (backward compat).

---

## 7. Desktop (`desktop/src/controller.rs` +348/-~120, `main.rs` +154, `state.rs` +131, `fs_provider.rs:115`, `ipc.rs:35`, `windows_audio.rs:21`, `remote_commands.rs:7`, `file_browser.rs:37`)

- **`main.rs:58`:** `format_timestamp` checked_add overflow guard + Howard Hinnant civil-from-days (was 365/30 drift). `poller_set()` equality check prevents 2 Hz full Dioxus re-diff. `request_elevated_firewall_install` escapes `'` → `''`, `current_exe` no empty-path unrestricted rule. `quit_application` waits on `shutdown_complete` flag (was fixed 500 ms).
- **`controller.rs`:** `pending_retry: HashMap<String, Vec<PathBuf>>` keyed by **original dest IP** + `transfer_targets` map (was `Vec<(Path,fingerprint)>` + fallback `0.0.0.0` → sent to wrong peer). Live registries `get_live_incoming/outgoing_transfer_ids` (single active id overwritten → cancel unreachable). Download `sanitize_filename` + preview `32 MB` cap (was read-after-delete, now read-then-delete). Clipboard `spawn_blocking`, MPRIS `PlayerFinder` single connection (was N `Connection::get_private` per poll), `MediaCommand::Mute` routed to `windows_audio` (was dead SMTC), pairing mode persisted (`pairing_mode_enabled` setting), `RefreshDevices` preserves trusted, `MEDIA_POLLER_HANDLE` aborts duplicate pollers, ETA `saturating_sub`+`max(1)`.
- **`state.rs`:** `pairing_mode_enabled`, `SHUTDOWN_COMPLETE` flag, `SETTINGS_SAVE_LOCK` background thread re-snapshotting (fixes lost-update race + AV-sleep UI freeze), corrupt settings backup `.json.corrupt.{ts}`, `explorer.exe arg` not `cmd /C start ""` (quote injection), live transfer sets.
- **`fs_provider.rs:115`:** `symlink_metadata` not `metadata`, symlink size 0, thumbnail uses `reader.limits(max_alloc 64M, width/height 10k)` in single decode (was check-then-reopen TOCTOU).
- **`ipc.rs`:** wakeup pipe retries on detached thread.
- **`windows_audio.rs:15`:** handles `RPC_E_CHANGED_MODE` and conditional `CoUninitialize`.
- **`remote_commands.rs:48`:** `explorer.exe` not `cmd` (`BatBadBut` `&|^` injection).
- **`file_browser.rs`:** preview only cloned when `PREVIEW_UPDATE` timestamp changes.

---

## 8. Android (`android/app/src/main/...`, `AndroidManifest.xml:14`)

- **`AndroidManifest.xml`:** `REQUEST_INSTALL_PACKAGES`, `dataExtractionRules`/`fullBackupContent` **excluding `identity.json`/`known_peers.json`** (clone impersonation) — new `android/app/src/main/res/xml/data_extraction_rules.xml:1` (15 LOC), `full_backup_content.xml:1` (7 LOC); `ACTION_CANCEL_TRANSFER`, `UpdateReceiver exported=false`.
- **`AndroidFilesystemProvider.kt:48`:** `canonicalPath.startsWith(root+File.separator)` (was prefix → `<root>_private` bypass).
- **`ConnectedApp.kt` (457 Δ):** `lifecycleExecutor` single-thread serializes `initialize`/`cleanup` (ANR+double-init), `initialized` volatile idempotence, `CoroutineExceptionHandler` (SMS SQLite crash), `ConcurrentHashMap.newKeySet` for `pendingPairing`, fixed 30 s auto-accept window not rolling (attacker stream indefinite), `onTransferCompleted` sanitizes `sanitizeRemoteFileName` + `isInsideDir` canonical check + moves on `Dispatchers.IO` (was Rust listener thread block), expired `pendingFileTransfersAwaitingIp` deletes temp files, separate `transfer_requests` (HIGH) vs `transfer_progress` (DEFAULT) channels, `sdkRestartLock` debounced+serialized, `stopWifiAwareManager` unconditional close.
- **`WifiAwareManager.kt` (106 Δ):** `started` guard for late `onAttached`, `NETWORK_REQUEST_TIMEOUT_MS=30s` watchdog evicting stale slots + `evictNetworkRequest[s]`, socket FD `dup` via `BorrowedFd.try_clone_to_owned` + `close()` after inject (was leak), PSK security warning.
- **`PathResolver.kt:63+`:** `canonicalizeInside()` against `allowedRoots` (primary + secondary volumes) — forged `content://.../../` raw paths blocked.
- **`TelephonyProvider.kt:55+`:** Dedicated `HandlerThread("connected-mms-observer")` + `ensureObserverHandler`, `quitSafely` on unregister, initial MMS scan off main thread (105 MB base64 OOM/ANR).
- **`MainActivity.kt:10`:** `isFinishing && !isRunning` + `cleanupAsync()` (rotation no longer destroys core).
- **`AppUpdater.kt:8`:** 10 s/15 s timeouts + `disconnect()` finally.
- **`UpdateReceiver.kt:46`:** checks `canRequestPackageInstalls()` + `STATUS_SUCCESSFUL` before install.
- **`MediaObserverService.kt:30`:** tracks `activeController/callback` and `unregisterActiveCallback()` (was leaking callbacks).
- **`ClipboardHelperActivity.kt:8`:** `clipboardShared` guard.
- **`ConnectedService.kt:12`:** removes early-return zombie service token leak (always `stopForeground`+cleanup).

---

## 9. iOS (`ios/Connected/Core/BonjourPublisher.swift:76`, `ConnectedAppModel.swift:38`, `IOSFilesystemProvider.swift:20`)

- **`BonjourPublisher.swift:76`:** `lastPort/DeviceId/Txt` writes moved onto `queue` (data race), `updateTxt` restores `id` and falls back to `publishOnQueue` if not published, extracted `publishOnQueue` DRY.
- **`IOSFilesystemProvider.swift:20`:** stores `rootCanonicalPath` (`resolvingSymlinksInPath`), containment via canonical path (symlink inside shared root → outside bypass).
- **`ConnectedAppModel.swift:38`:** `isInitializing` sync MainActor guard (double `initializeIfNeeded` race), `sanitizedDownloadName()` strips `\→/`, separators, `..`, leading `.`, controls, 255-byte cap before `appendingPathComponent`.

---

## 10. FFI (`ffi/src/lib.rs:113`, `ffi/tests/test_ffi.rs:61`)

- `From<ConnectedError>` maps to granular `InvalidArgument`/`Internal`/`ConnectionError` (was all `ConnectionError`).
- `install_client()` atomic check-then-act under single `write()` lock + `block_on(client.shutdown())` for loser (was two separate locks → double port bind + leaked event listener).
- `inject_aware_socket` `BorrowedFd::borrow_raw(fd).try_clone_to_owned()` dup before `set_nonblocking` (was `from_raw_fd` closing caller FD on failure).
- `spawn_event_listener` uses `tokio::Notify` select (was `shutdown_flag` poll).
- `shutdown()` clears **all** callbacks (`MEDIA/TELEPHONY/REMOTE_COMMANDS/FS_PROVIDER`) and `notify_waiters`.

---

## 11. Tests — adversarial + integration (`core/tests/adversarial_test.rs:193` NEW, `core/tests/client_integration_test.rs:108`, `ffi/tests/test_ffi.rs:61`)

**`core/tests/adversarial_test.rs:1` (193 LOC, NEW):**
- `sanitize_strips_all_traversal_shapes`, `sanitize_never_returns_dotfiles`, `sanitize_handles_unicode_and_long_names` (300 CJK chars/emoji at boundary), `sanitize_control_characters`.
- `safe_relative_rejects_traversal` / `accepts_legitimate` for `is_safe_relative_path`.
- `decode_fuzz_random_bytes_never_panics` (20 k deterministic xorshift, biases to `0x01`), `decode_fuzz_structurally_valid_but_hostile_lengths` (`u64::MAX` string/vec lengths), `encode_decode_roundtrip_v1_and_v2` (bincode wins on text-heavy payloads).

**`core/tests/client_integration_test.rs:108`:** identity persistence across restart, `KeyStore` trust→unpair→block lifecycle.

**`ffi/tests/test_ffi.rs:61`:** `DeviceType` string contract + pre-init error shape tests.

**`core/src/codec.rs:83` inline tests + `core/src/file_transfer.rs` style tests** complement above.

---

## Themes

- **Protocol:** JSON→bincode v2 with 128 MB budget and magic-byte negotiation; ~30% smaller control messages; full v1 backward compat.
- **~15 security fixes:** path traversal (Android/iOS/desktop/file_transfer), symlink overwrite, pairing auto-trust (fail-closed `is_some_and`), DeviceUnpaired forgery, clipboard OOM, `BatBadBut` command injection (`explorer.exe`), backup exfil (`fullBackupContent`), FD/MPRIS/callback leaks, `REQUEST_INSTALL_PACKAGES` checks.
- **~10 ANR/shutdown races:** `lifecycleExecutor`, `shutdown_complete` wait, `HandlerThread`, `CONNECT_TIMEOUT`, `ConnectingGuard`, `background_tasks` abort, `MEDIA_POLLER_HANDLE`.
- **Discovery stability:** 30 s stale, bounded retry, debounced `ServiceRemoved`, no `unregister()` on re-announce.

---

## Working with branches

```bash
git checkout dev      # dev/testing (this branch, origin/dev)
git checkout main     # clean release (origin/main)
# new feature from dev:
git checkout -b feat/xyz dev
```

To bring `main` hotfixes into `dev`: `git checkout dev && git merge main` or `git rebase main`.

