fn main() {
    let Ok(target_os) = std::env::var("CARGO_CFG_TARGET_OS") else {
        return;
    };
    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        // Use absolute path to ensure windres finds it
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let icon_path = std::path::Path::new(&manifest_dir)
            .join("assets")
            .join("icon.ico");

        eprintln!("Icon path: {:?}", icon_path);
        if icon_path.exists() {
            if let Some(path_str) = icon_path.to_str() {
                res.set_icon(path_str);
            } else {
                eprintln!(
                    "cargo:warning=Icon path contains invalid UTF-8, skipping icon embedding"
                );
            }
        } else {
            eprintln!(
                "cargo:warning=Icon not found at {:?}, continuing without icon",
                icon_path
            );
        }

        // Dioxus/WebView2 usually needs a manifest for high DPI and controls
        res.set_manifest(r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <!-- Windows 10/11 -->
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
    </application>
  </compatibility>
</assembly>
"#);

        // Point to the correct windres tool for cross-compilation
        if std::env::consts::OS == "linux" {
            res.set_toolkit_path("/usr/bin");
            res.set_windres_path("x86_64-w64-mingw32-windres");
        }

        if let Err(e) = res.compile() {
            eprintln!(
                "cargo:warning=Failed to compile Windows resources: {}. Continuing without them.",
                e
            );
        }
    }
}
