$ErrorActionPreference = "Stop"

# Microsoft Edge WebView2 Runtime (Evergreen) Bootstrapper.
# This is a small installer that downloads the runtime if needed.
# Source: Microsoft (fwlink).
$url = "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
$outFile = Join-Path $PSScriptRoot "MicrosoftEdgeWebView2Setup.exe"

Write-Host "Downloading WebView2 bootstrapper..."
Write-Host "  URL: $url"
Write-Host "  Out: $outFile"

Invoke-WebRequest -Uri $url -OutFile $outFile

if (-not (Test-Path $outFile)) {
  throw "Download failed: $outFile was not created."
}

Write-Host "Done."
