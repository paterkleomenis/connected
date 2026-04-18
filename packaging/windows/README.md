# Windows packaging targets

Connected now supports building desktop binaries for both Windows x64 and Windows ARM64.

- `x86_64-pc-windows-msvc`: produces the MSI installer (`connected-desktop-windows-x64.msi`).
- `aarch64-pc-windows-msvc`: produces a standalone executable artifact (`connected-desktop-windows-arm64.exe`).

## Why ARM64 is EXE-only for now

The WiX installer file at `packaging/windows/installer.wxs` is currently wired to
the host build output layout (`target\release\...`) and x64 WebView2 loader lookup.
To keep release automation stable, ARM64 builds are published as `.exe` artifacts.

If we decide to ship an ARM64 MSI later, we should add a second WiX template with
target-specific source paths and the ARM64 WebView2 loader handling.
