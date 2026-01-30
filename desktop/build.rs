use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let Ok(target_os) = std::env::var("CARGO_CFG_TARGET_OS") else {
        return;
    };
    if target_os == "windows" {
        // Attempt to copy WebView2Loader.dll to the target directory
        copy_webview2_loader();

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

fn copy_webview2_loader() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Navigate up to the 'build' directory (target/release/build)
    // Structure is usually: target/release/build/<package-name>-<hash>/out
    let Some(build_dir) = out_dir.parent().and_then(|p| p.parent()) else {
        return;
    };

    // Look for webview2-com-sys-* directory
    let entries = match fs::read_dir(build_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let arch_dir = match arch.as_str() {
        "x86_64" => "x64",
        "x86" => "x86",
        "aarch64" => "arm64",
        _ => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("webview2-com-sys-") {
            let src_path = entry
                .path()
                .join("out")
                .join(arch_dir)
                .join("WebView2Loader.dll");
            if src_path.exists() {
                // Target directory is the parent of the build directory (target/release)
                if let Some(target_dir) = build_dir.parent() {
                    let dest_path = target_dir.join("WebView2Loader.dll");
                    if let Err(e) = fs::copy(&src_path, &dest_path) {
                        println!("cargo:warning=Failed to copy WebView2Loader.dll: {}", e);
                    }
                }
                return;
            }
        }
    }

    // If we didn't find it, leave a loud breadcrumb: WiX will hard-fail later without it.
    println!(
        "cargo:warning=WebView2Loader.dll not found under {:?}; MSI packaging will fail unless it is copied into the target dir.",
        build_dir
    );
}
