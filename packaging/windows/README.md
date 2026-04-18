# Windows packaging targets

Connected now supports Windows MSIX packaging for both x64 and ARM64.

- `x86_64-pc-windows-msvc`: produces `connected-desktop-windows-x64.msix`.
- `aarch64-pc-windows-msvc`: produces `connected-desktop-windows-arm64.msix`.
- Release workflow bundles both into `connected-desktop-windows.msixbundle`.

## Local MSIX build

Use:

- `just build-windows-msix x64 release`
- `just build-windows-msix arm64 release`

or directly:

- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-msix.ps1 -Arch x64 -Profile release`
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-msix.ps1 -Arch arm64 -Profile release`

Output paths:

- `target/msix/x64/connected-desktop-windows-x64.msix`
- `target/msix/arm64/connected-desktop-windows-arm64.msix`
- `target/msix/connected-desktop-windows.msixbundle` (created in CI/release workflow)

Note: produced MSIX files are unsigned. Sign them with `signtool` for trusted installation flows.

WiX/MSI packaging is no longer part of the release assets workflow.
