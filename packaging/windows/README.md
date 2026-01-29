# Windows Installer Notes

## WebView2 Runtime dependency

The Windows desktop app uses WebView2. The browser app ("Microsoft Edge") is **not** required, but the **Microsoft Edge WebView2 Runtime** is required.

This installer includes `MicrosoftEdgeWebView2Setup.exe` and runs it during install (silent) to ensure the runtime is present.

## Preparing the bootstrapper

Before building the MSI, download the WebView2 Evergreen bootstrapper into this folder:

- Run `powershell -ExecutionPolicy Bypass -File packaging\\windows\\fetch-webview2-bootstrapper.ps1`

This will create `packaging/windows/MicrosoftEdgeWebView2Setup.exe`, which `packaging/windows/installer.wxs` expects.

