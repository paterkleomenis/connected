#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(non_snake_case)]

mod components;
mod controller;
mod fs_provider;
mod ipc;
mod mpris_server;
mod proximity;
mod state;
mod utils;

use state::{is_discovery_active, is_sdk_initialized};

use state::{
    ThemeModeSetting, get_clipboard_sync_enabled, get_device_name_setting,
    get_media_enabled_setting, get_theme_mode_setting, set_clipboard_sync_enabled,
    set_media_enabled_setting, set_theme_mode_setting,
};

use components::{DeviceCard, FileBrowser, FileDialog, Icon, IconType};
use connected_core::telephony::{ActiveCallState, CallAction};
use connected_core::{MediaCommand, UpdateInfo};
use controller::{AppAction, app_controller};
use dioxus::prelude::*;

use state::*;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::debug;
use utils::{get_hostname, get_system_clipboard};

fn format_timestamp(ts: u64) -> String {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let diff_secs = (now.saturating_sub(ts)) / 1000;

    if diff_secs < 60 {
        "Just now".to_string()
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86400 {
        format!("{}h ago", diff_secs / 3600)
    } else if diff_secs < 604800 {
        format!("{}d ago", diff_secs / 86400)
    } else {
        let secs = ts / 1000;
        let datetime = UNIX_EPOCH + Duration::from_secs(secs);
        if let Ok(dur) = datetime.duration_since(UNIX_EPOCH) {
            let days = dur.as_secs() / 86400;
            let years = 1970 + days / 365;
            let remaining = days % 365;
            let months = remaining / 30 + 1;
            let day = remaining % 30 + 1;
            format!("{}/{}/{}", months, day, years % 100)
        } else {
            "Unknown".to_string()
        }
    }
}

#[cfg(target_os = "linux")]
mod tray {
    use dioxus::desktop::tao::window::Window;
    use std::sync::Arc;

    pub struct ConnectedTray {
        pub window: Arc<Window>,
    }

    impl ksni::Tray for ConnectedTray {
        fn id(&self) -> String {
            "connected-desktop".to_string()
        }

        fn title(&self) -> String {
            "Connected".to_string()
        }

        fn icon_name(&self) -> String {
            "connected-app-logo".to_string()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let mut icons = Vec::new();

            for size in [22u32, 32, 64] {
                let rgba = super::render_connected_tray_icon_rgba(size);
                let mut argb = Vec::with_capacity((size * size * 4) as usize);
                for px in rgba.chunks_exact(4) {
                    let r = px[0];
                    let g = px[1];
                    let b = px[2];
                    let a = px[3];
                    argb.push(a);
                    argb.push(r);
                    argb.push(g);
                    argb.push(b);
                }

                icons.push(ksni::Icon {
                    width: size as i32,
                    height: size as i32,
                    data: argb,
                });
            }

            icons
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            vec![
                StandardItem {
                    label: "Show Connected".to_string(),
                    activate: Box::new(|tray: &mut Self| {
                        tray.window.set_visible(true);
                        tray.window.set_minimized(false);
                        tray.window.set_focus();
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Hide Connected".to_string(),
                    activate: Box::new(|tray: &mut Self| {
                        tray.window.set_visible(false);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit".to_string(),
                    activate: Box::new(|_: &mut Self| {
                        std::process::exit(0);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }
}

use image::ImageReader;
use std::io::Cursor;

#[derive(Clone, Debug, PartialEq)]
struct MmsInlineImage {
    data_url: String,
    base64_data: String,
    content_type: String,
    filename: Option<String>,
}

fn attachment_preview_image(
    attachments: &[connected_core::telephony::MmsAttachment],
) -> Option<MmsInlineImage> {
    attachments.iter().find_map(|attachment| {
        if !attachment.content_type.starts_with("image/") {
            return None;
        }

        let data = attachment.data.as_ref()?;
        if data.is_empty() {
            return None;
        }

        Some(MmsInlineImage {
            data_url: format!("data:{};base64,{}", attachment.content_type, data),
            base64_data: data.clone(),
            content_type: attachment.content_type.clone(),
            filename: attachment.filename.clone(),
        })
    })
}

fn suggested_mms_image_filename(image: &MmsInlineImage) -> String {
    let fallback_extension = mime_guess::get_mime_extensions_str(&image.content_type)
        .and_then(|extensions| extensions.first().copied())
        .unwrap_or("jpg");

    if let Some(raw_name) = image.filename.as_deref() {
        let trimmed = raw_name.trim();
        if !trimmed.is_empty() {
            let leaf_name = Path::new(trimmed)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(trimmed);
            let safe_name = connected_core::file_transfer::sanitize_filename(leaf_name);
            if !safe_name.is_empty() && safe_name != "unnamed" {
                if Path::new(&safe_name).extension().is_some() {
                    return safe_name;
                }
                return format!("{safe_name}.{fallback_extension}");
            }
        }
    }

    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("mms-image-{suffix}.{fallback_extension}")
}

fn save_mms_image_to_downloads(
    image: &MmsInlineImage,
    configured_download_directory: &str,
) -> Result<PathBuf, String> {
    use base64::Engine as _;

    let image_bytes = base64::engine::general_purpose::STANDARD
        .decode(image.base64_data.as_bytes())
        .map_err(|err| format!("Invalid image payload: {err}"))?;

    let target_dir = if configured_download_directory.trim().is_empty() {
        dirs::download_dir().unwrap_or_else(|| PathBuf::from("."))
    } else {
        PathBuf::from(configured_download_directory)
    };

    std::fs::create_dir_all(&target_dir)
        .map_err(|err| format!("Could not prepare download directory: {err}"))?;

    let preferred_name = suggested_mms_image_filename(image);
    let preferred_path = Path::new(&preferred_name);
    let stem = preferred_path
        .file_stem()
        .and_then(|part| part.to_str())
        .unwrap_or("mms-image")
        .to_string();
    let extension = preferred_path
        .extension()
        .and_then(|part| part.to_str())
        .unwrap_or("")
        .to_string();

    let mut candidate = target_dir.join(&preferred_name);
    let mut counter = 1u32;
    while candidate.exists() {
        let suffix = if extension.is_empty() {
            String::new()
        } else {
            format!(".{extension}")
        };
        candidate = target_dir.join(format!("{stem}-{counter}{suffix}"));
        counter += 1;
    }

    std::fs::write(&candidate, image_bytes)
        .map_err(|err| format!("Could not write image file: {err}"))?;

    Ok(candidate)
}

fn load_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    let icon_bytes = include_bytes!("../assets/logo.png");
    let reader = ImageReader::new(Cursor::new(icon_bytes))
        .with_guessed_format()
        .ok()?;
    let image = reader.decode().ok()?;
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    dioxus::desktop::tao::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
}

fn sdf_rounded_rect(px: f32, py: f32, cx: f32, cy: f32, half_w: f32, half_h: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (half_w - r);
    let qy = (py - cy).abs() - (half_h - r);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn sdf_to_alpha(distance: f32, aa: f32) -> f32 {
    ((aa - distance) / aa).clamp(0.0, 1.0)
}

fn blend_rgba(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let src_a = src[3];
    let dst_a = dst[3];
    let out_a = src_a + dst_a * (1.0 - src_a);

    if out_a <= f32::EPSILON {
        return [0.0, 0.0, 0.0, 0.0];
    }

    let out_r = (src[0] * src_a + dst[0] * dst_a * (1.0 - src_a)) / out_a;
    let out_g = (src[1] * src_a + dst[1] * dst_a * (1.0 - src_a)) / out_a;
    let out_b = (src[2] * src_a + dst[2] * dst_a * (1.0 - src_a)) / out_a;

    [out_r, out_g, out_b, out_a]
}

fn render_connected_tray_icon_rgba(size: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;

    let padding = (s * 0.12).max(1.5);
    let chip_side = (s - 2.0 * padding).max(1.0);
    let chip_center = s * 0.5;
    let chip_half = chip_side * 0.5;
    let chip_radius = (chip_side * 0.28).min(chip_half - 0.5).max(2.2);

    let border_width = (s * 0.06).max(1.0);
    let inner_half = (chip_half - border_width).max(0.5);
    let inner_radius = (chip_radius - border_width).max(1.0);

    let aa = 1.0;
    let samples = [(0.25f32, 0.25f32), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];

    for y in 0..size {
        for x in 0..size {
            let mut pixel = [0.0f32; 4];

            for (sx, sy) in samples {
                let px = x as f32 + sx;
                let py = y as f32 + sy;

                let outer_d = sdf_rounded_rect(
                    px,
                    py,
                    chip_center,
                    chip_center,
                    chip_half,
                    chip_half,
                    chip_radius,
                );
                let outer_cov = sdf_to_alpha(outer_d, aa);

                let inner_d = sdf_rounded_rect(
                    px,
                    py,
                    chip_center,
                    chip_center,
                    inner_half,
                    inner_half,
                    inner_radius,
                );
                let inner_cov = sdf_to_alpha(inner_d, aa);

                let border_cov = (outer_cov - inner_cov).clamp(0.0, 1.0);

                let mut sample_px = [0.0f32, 0.0, 0.0, 0.0];
                // Dark chip fill
                sample_px = blend_rgba(sample_px, [0.06, 0.06, 0.06, inner_cov * 0.92]);
                // Light border
                sample_px = blend_rgba(sample_px, [1.0, 1.0, 1.0, border_cov * 0.5]);

                // Logo geometry (based on IconType::Logo viewBox 116x116)
                let ux = ((px - padding) / chip_side) * 116.0;
                let uy = ((py - padding) / chip_side) * 116.0;
                let dx = ux - 58.0;
                let dy = uy - 58.0;
                let dist = (dx * dx + dy * dy).sqrt();

                let dot_cov = sdf_to_alpha(dist - 9.0, 1.15);
                let mut arc_cov = 0.0;
                let angle = dy.atan2(dx);
                let gap_start = 0.608;
                let gap_end = 2.533;
                if !(angle > gap_start && angle < gap_end) {
                    arc_cov = sdf_to_alpha((dist - 38.0).abs() - 8.0, 1.2);
                }
                let logo_cov = dot_cov.max(arc_cov) * inner_cov;

                sample_px = blend_rgba(sample_px, [1.0, 1.0, 1.0, logo_cov]);

                pixel[0] += sample_px[0];
                pixel[1] += sample_px[1];
                pixel[2] += sample_px[2];
                pixel[3] += sample_px[3];
            }

            let inv = 1.0 / samples.len() as f32;
            let i = ((y * size + x) * 4) as usize;
            rgba[i] = (pixel[0] * inv * 255.0).round() as u8;
            rgba[i + 1] = (pixel[1] * inv * 255.0).round() as u8;
            rgba[i + 2] = (pixel[2] * inv * 255.0).round() as u8;
            rgba[i + 3] = (pixel[3] * inv * 255.0).round() as u8;
        }
    }

    rgba
}

#[cfg(target_os = "windows")]
fn load_tray_icon() -> Option<dioxus::desktop::trayicon::Icon> {
    let size = 64;
    let rgba = render_connected_tray_icon_rgba(size);
    dioxus::desktop::trayicon::Icon::from_rgba(rgba, size, size).ok()
}

/// Command-line flag that the elevated subprocess receives.
#[cfg(target_os = "windows")]
const FIREWALL_INSTALL_ARG: &str = "--install-firewall-rules";

/// Declarative definition of a single firewall rule.  Both the "check" and
/// "create" paths derive from this same array, so there is exactly one place
/// to add or change a rule.
#[cfg(target_os = "windows")]
struct FirewallRuleDef {
    name: &'static str,
    direction: windows_firewall::Direction,
    description: &'static str,
    /// If set, constrains local ports.
    local_ports: Option<[u16; 1]>,
    /// If set, constrains remote ports.
    remote_ports: Option<[u16; 1]>,
}

#[cfg(target_os = "windows")]
const FIREWALL_RULES: [FirewallRuleDef; 4] = {
    use windows_firewall::Direction as Dir;
    [
        FirewallRuleDef {
            name: "Connected Desktop (mDNS) - Inbound",
            direction: Dir::In,
            description: "Allow Connected Desktop to receive mDNS traffic for local network discovery.",
            local_ports: Some([5353]),
            remote_ports: None,
        },
        FirewallRuleDef {
            name: "Connected Desktop (mDNS) - Outbound",
            direction: Dir::Out,
            description: "Allow Connected Desktop to send mDNS queries for local network discovery.",
            local_ports: None,
            remote_ports: Some([5353]),
        },
        FirewallRuleDef {
            name: "Connected Desktop (QUIC) - Inbound",
            direction: Dir::In,
            description: "Allow Connected Desktop to receive incoming connections from paired devices.",
            local_ports: None,
            remote_ports: None,
        },
        FirewallRuleDef {
            name: "Connected Desktop (QUIC) - Outbound",
            direction: Dir::Out,
            description: "Allow Connected Desktop to connect to paired devices.",
            local_ports: None,
            remote_ports: None,
        },
    ]
};

/// Check whether all firewall rules already exist with the expected
/// configuration.  This does **not** require admin privileges.
#[cfg(target_os = "windows")]
fn firewall_rules_ok(exe_path: &str) -> bool {
    use windows_firewall::{Action, Protocol, get_rule};

    FIREWALL_RULES.iter().all(|def| {
        let Ok(rule) = get_rule(def.name) else {
            return false;
        };
        *rule.enabled()
            && *rule.action() == Action::Allow
            && *rule.direction() == def.direction
            && rule.protocol().as_ref() == Some(&Protocol::Udp)
            && rule
                .application_name()
                .as_deref()
                .is_some_and(|app| app.eq_ignore_ascii_case(exe_path))
    })
}

/// Create / update all firewall rules.  **Requires admin privileges.**
#[cfg(target_os = "windows")]
fn create_firewall_rules(exe_path: &str) -> bool {
    use windows_firewall::{Action, FirewallRule, Protocol};

    let mut ok = true;
    for def in &FIREWALL_RULES {
        let mut rule = FirewallRule::builder()
            .name(def.name)
            .action(Action::Allow)
            .direction(def.direction)
            .enabled(true)
            .description(def.description)
            .protocol(Protocol::Udp)
            .application_name(exe_path)
            .build();

        if let Some(ports) = def.local_ports {
            rule.set_local_ports(Some(
                ports
                    .into_iter()
                    .map(windows_firewall::Port::from)
                    .collect(),
            ));
        }
        if let Some(ports) = def.remote_ports {
            rule.set_remote_ports(Some(
                ports
                    .into_iter()
                    .map(windows_firewall::Port::from)
                    .collect(),
            ));
        }

        if let Err(e) = rule.add_or_update() {
            eprintln!("Failed to add/update firewall rule \"{}\": {}", def.name, e);
            ok = false;
        }
    }
    ok
}

/// Spawn an elevated copy of the program that only installs firewall rules,
/// wait for it to finish, and return whether it succeeded.
///
/// Uses `powershell Start-Process -Verb RunAs` to trigger the UAC prompt,
/// avoiding any need for `unsafe` Win32 FFI.
#[cfg(target_os = "windows")]
fn request_elevated_firewall_install(exe_path: &std::path::Path) -> bool {
    let exe_str = exe_path.to_string_lossy();

    // Start-Process -Verb RunAs triggers UAC.
    // -Wait blocks until the elevated process exits.
    // -WindowStyle Hidden should prevent a console window flash,
    //  but because it doesn't, we handle it below
    let ps_command = format!(
        "Start-Process -FilePath '{}' -ArgumentList '{}' -Verb RunAs -Wait -WindowStyle Hidden",
        exe_str, FIREWALL_INSTALL_ARG,
    );

    // Actual window hiding
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let powershell_exe = format!(
        "{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
        system_root
    );

    match std::process::Command::new(powershell_exe)
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-Command", &ps_command])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(e) => {
            tracing::warn!("Failed to launch PowerShell for UAC elevation: {}", e);
            false
        }
    }
}

/// Entry point for the `--install-firewall-rules` elevated subprocess.
/// Creates all firewall rules and exits immediately
#[cfg(target_os = "windows")]
fn run_firewall_install_and_exit() -> ! {
    let exe_path = std::env::current_exe().unwrap_or_default();
    let code = if create_firewall_rules(&exe_path.to_string_lossy()) {
        0
    } else {
        1
    };
    std::process::exit(code);
}

/// Top-level firewall orchestrator called from `main()`.
///
/// If rules already exist and look correct → skip.
/// If not, spawn an elevated subprocess to create them via UAC.
#[cfg(target_os = "windows")]
fn ensure_firewall_rules() {
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("Cannot determine exe path for firewall rules: {}", e);
            return;
        }
    };
    let exe_path_str = exe_path.to_string_lossy();

    // Fast path: all rules already exist and match — nothing to do.
    if firewall_rules_ok(&exe_path_str) {
        tracing::debug!("Firewall rules already configured correctly");
        return;
    }

    tracing::info!(
        "Firewall rules are missing or incorrect — requesting elevation to install them"
    );

    if request_elevated_firewall_install(&exe_path) && firewall_rules_ok(&exe_path_str) {
        tracing::info!("Firewall rules installed successfully");
    } else {
        tracing::warn!(
            "Could not install firewall rules (UAC declined or subprocess failed). \
             If you experience connectivity issues, manually allow \"{}\" through \
             Windows Firewall for UDP traffic.",
            exe_path_str
        );
    }
}

fn main() {
    // Handle the elevated-subprocess entry point before doing anything else.
    // When launched with --install-firewall-rules the process creates the
    // firewall rules and exits immediately
    #[cfg(target_os = "windows")]
    if std::env::args().any(|a| a == FIREWALL_INSTALL_ARG) {
        run_firewall_install_and_exit();
    }

    let instance_name = if cfg!(target_os = "macos") {
        let mut path = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        path.push("connected");
        let _ = std::fs::create_dir_all(&path);
        path.push("connected-desktop-app.lock");
        path.to_string_lossy().into_owned()
    } else {
        "connected-desktop-app".to_string()
    };

    let instance_result = single_instance::SingleInstance::new(&instance_name);

    let _instance = match instance_result {
        Ok(instance) => {
            if !instance.is_single() {
                eprintln!("Another instance is already running. Waking it up...");
                ipc::send_wakeup_signal();
                std::process::exit(0);
            }
            Some(instance)
        }
        Err(e) => {
            eprintln!("Warning: Failed to acquire single instance lock: {}", e);
            eprintln!("Multiple instances may run. Proceeding anyway...");
            // Best-effort: continue without single-instance protection
            None
        }
    };

    // Explicitly select the Rustls crypto provider to avoid runtime ambiguity.
    if let Err(err) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Failed to install rustls ring provider: {err:?}");
    }

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    #[cfg(target_os = "windows")]
    ensure_firewall_rules();

    // Platform-specific window settings
    let decorations = cfg!(target_os = "windows") || cfg!(target_os = "macos");
    let transparent = !cfg!(target_os = "windows") && !cfg!(target_os = "macos");

    let data_dir = dirs::data_local_dir().map(|d| d.join("connected"));
    if let Some(d) = data_dir.as_ref().filter(|d| !d.exists()) {
        let _ = std::fs::create_dir_all(d);
    }

    #[allow(unused_mut)]
    let mut config = dioxus::desktop::Config::new().with_window(
        dioxus::desktop::WindowBuilder::new()
            .with_title("Connected")
            .with_inner_size(dioxus::desktop::LogicalSize::new(1100.0, 700.0))
            .with_decorations(decorations)
            .with_transparent(transparent)
            .with_window_icon(load_icon()),
    );

    // Set up menu bar on macOS
    #[cfg(target_os = "macos")]
    {
        use dioxus::desktop::muda::{Menu, PredefinedMenuItem, Submenu};

        let menu = Menu::new();
        let app_menu = Submenu::new("Connected", true);
        app_menu
            .append_items(&[
                &PredefinedMenuItem::about(None, None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(None),
            ])
            .expect("Failed to build app menu");

        let window_menu = Submenu::new("Window", true);
        window_menu
            .append_items(&[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::maximize(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::close_window(None),
            ])
            .expect("Failed to build window menu");

        menu.append_items(&[&app_menu, &window_menu])
            .expect("Failed to build menu");

        config = config.with_menu(Some(menu));
    }

    #[cfg(not(target_os = "macos"))]
    {
        config = config.with_menu(None);
    }

    config = config.with_disable_context_menu(true);

    if let Some(d) = data_dir {
        config = config.with_data_directory(d);
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        config = config.with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowHides);
    }

    LaunchBuilder::desktop().with_cfg(config).launch(App);
}

/// Consolidated UI-side media state.  Replaces four separate `use_signal`s that
/// were all derived from the single `get_current_media()` global and updated
/// together on every poll tick.
#[derive(Clone, Debug, PartialEq)]
struct CurrentMediaUi {
    title: String,
    artist: String,
    playing: bool,
    source_device_id: String,
}

impl Default for CurrentMediaUi {
    fn default() -> Self {
        Self {
            title: "Not Playing".to_string(),
            artist: String::new(),
            playing: false,
            source_device_id: "local".to_string(),
        }
    }
}

fn App() -> Element {
    let window = dioxus::desktop::window();
    use_hook(move || {
        static SPUN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SPUN.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let window = window.clone();
            dioxus::prelude::spawn(async move {
                ipc::listen_for_wakeups(window).await;
            });
        }
    });

    // UI State
    let mut local_device_name =
        use_signal(|| get_device_name_setting().unwrap_or_else(get_hostname));
    let local_device_ip = use_signal(String::new);
    let mut devices_list = use_signal(Vec::<DeviceInfo>::new);
    let mut selected_device = use_signal(|| None::<DeviceInfo>);
    let mut active_tab = use_signal(|| "devices".to_string());
    let mut device_detail_tab = use_signal(|| "clipboard".to_string());
    let mut transfer_status = use_signal(|| TransferStatus::Idle);
    let mut clipboard_sync_enabled = use_signal(get_clipboard_sync_enabled);
    let mut notifications = use_signal(Vec::<Notification>::new);
    let mut show_send_dialog = use_signal(|| false);
    let mut send_dialog_is_folder = use_signal(|| false);
    let mut send_target_device = use_signal(|| None::<DeviceInfo>);
    let mut clipboard_text = use_signal(String::new);
    let mut show_rename_dialog = use_signal(|| false);
    let mut rename_text = use_signal(String::new);

    // Note: discovery_active is now tracked via global state in state.rs
    // (is_sdk_initialized() and is_discovery_active())
    let mut media_enabled = use_signal(get_media_enabled_setting);
    // Fix #17: Consolidate four separate media signals into one struct signal.
    // These were all derived from the same `get_current_media()` global state
    // and updated together in the polling loop — four signals for one source.
    let mut current_media = use_signal(CurrentMediaUi::default);

    // Pairing State
    let mut pairing_mode = use_signal(|| *get_pairing_mode_state().lock_or_recover());
    let mut pairing_requests = use_signal(Vec::<PairingRequest>::new);
    let mut file_transfer_requests = use_signal(Vec::<FileTransferRequest>::new);

    // Telephony State
    let mut phone_sub_tab = use_signal(|| "messages".to_string());
    let mut phone_contacts = use_signal(Vec::<connected_core::telephony::Contact>::new);
    let mut phone_conversations = use_signal(Vec::<connected_core::telephony::Conversation>::new);
    let mut phone_call_log = use_signal(Vec::<connected_core::telephony::CallLogEntry>::new);
    let mut selected_conversation = use_signal(|| None::<String>);
    let mut phone_messages = use_signal(Vec::<connected_core::telephony::SmsMessage>::new);
    let mut last_message_count = use_signal(|| 0usize);
    let mut sms_compose_text = use_signal(String::new);
    let mut active_call = use_signal(|| None::<connected_core::telephony::ActiveCall>);
    let mut update_info = use_signal(|| None::<UpdateInfo>);

    // Auto-sync settings (loaded from persistent storage)
    let mut auto_sync_messages = use_signal(get_auto_sync_messages);
    let mut auto_sync_calls = use_signal(get_auto_sync_calls);
    let mut auto_sync_contacts = use_signal(get_auto_sync_contacts);
    let mut notifications_enabled = use_signal(get_notifications_enabled_setting);
    let mut theme_mode = use_signal(get_theme_mode_setting);
    let mut open_mms_image = use_signal(|| None::<MmsInlineImage>);
    let mut download_directory = use_signal(|| {
        get_download_directory_setting().unwrap_or_else(|| {
            dirs::download_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .display()
                .to_string()
        })
    });

    let local_device_icon = if cfg!(target_os = "linux") {
        IconType::Linux
    } else if cfg!(target_os = "windows") {
        IconType::Windows
    } else if cfg!(target_os = "macos") {
        IconType::Macos
    } else {
        IconType::Desktop
    };

    // The Controller
    let action_tx =
        use_coroutine(
            move |rx: UnboundedReceiver<AppAction>| async move { app_controller(rx).await },
        );
    let (mpris_tx, mpris_rx) = std::sync::mpsc::channel();
    if mpris_server::init_mpris(mpris_tx) {
        spawn(async move {
            loop {
                while let Ok(command) = mpris_rx.try_recv() {
                    action_tx.send(AppAction::ControlRemoteMedia(command));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }

    // Start Init and initialize settings-based features
    use_effect(move || {
        action_tx.send(AppAction::Init);

        // Start media control if it was enabled in saved settings
        if get_media_enabled_setting() {
            action_tx.send(AppAction::ToggleMediaControl {
                enabled: true,
                notify: false,
            });
        }

        // Poll for Bluetooth/Wi-Fi state changes
        let action_tx_clone = action_tx;
        spawn(async move {
            // Helper to check states
            async fn check_adapters() -> (bool, bool) {
                #[cfg(target_os = "linux")]
                {
                    let bt_status = tokio::process::Command::new("rfkill")
                        .arg("list")
                        .arg("bluetooth")
                        .output()
                        .await;
                    let bt_on = if let Ok(output) = bt_status {
                        let out = String::from_utf8_lossy(&output.stdout);
                        !out.contains("Soft blocked: yes") && !out.contains("Hard blocked: yes")
                    } else {
                        true
                    };

                    let wifi_status = tokio::process::Command::new("nmcli")
                        .arg("radio")
                        .arg("wifi")
                        .output()
                        .await;
                    let wifi_on = if let Ok(output) = wifi_status {
                        let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        out == "enabled"
                    } else {
                        true
                    };
                    (bt_on, wifi_on)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    (true, true)
                }
            }

            // Init state
            let (mut last_bt_state, mut last_wifi_state) = check_adapters().await;

            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;

                let (bt_on, wifi_on) = check_adapters().await;

                let changed = bt_on != last_bt_state || wifi_on != last_wifi_state;

                if changed {
                    debug!(
                        "Adapter state changed (BT: {} -> {}, WiFi: {} -> {})",
                        last_bt_state, bt_on, last_wifi_state, wifi_on
                    );
                    action_tx_clone.send(AppAction::RefreshDiscovery);
                }

                last_bt_state = bt_on;
                last_wifi_state = wifi_on;
            }
        });
    });

    #[cfg(target_os = "windows")]
    {
        use dioxus::desktop::trayicon::menu::{Menu, MenuItem, PredefinedMenuItem};
        use dioxus::desktop::use_window;

        let window = use_window();
        let window = window.window.clone();

        let (show_id, hide_id, quit_id) = use_hook(|| {
            let menu = Menu::new();
            let show = MenuItem::new("Show Connected", true, None);
            let hide = MenuItem::new("Hide Connected", true, None);
            let quit = MenuItem::new("Quit", true, None);

            menu.append_items(&[&show, &hide, &PredefinedMenuItem::separator(), &quit])
                .expect("Failed to build tray menu");

            dioxus::desktop::trayicon::init_tray_icon(menu, load_tray_icon());

            (show.id().clone(), hide.id().clone(), quit.id().clone())
        });

        // NOTE: Dioxus installs a global `muda::MenuEvent` handler. Tray menu clicks are delivered
        // through that same event stream on Windows, so `use_muda_event_handler` is the reliable hook.
        dioxus::desktop::use_muda_event_handler(move |event| {
            if event.id == show_id {
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
            } else if event.id == hide_id {
                window.set_visible(false);
            } else if event.id == quit_id {
                std::process::exit(0);
            }
        });
    }

    #[cfg(target_os = "linux")]
    {
        use dioxus::desktop::use_window;
        use ksni::TrayMethods;

        let window = use_window();

        // Initialize tray icon
        use_hook(|| {
            let tray = tray::ConnectedTray {
                window: window.window.clone(),
            };
            // Spawn the tray service using the ksni async API
            tokio::spawn(async move {
                match tray.spawn().await {
                    Ok(_handle) => {
                        debug!("System tray initialized");
                    }
                    Err(e) => {
                        tracing::error!("Failed to initialize system tray: {:?}", e);
                    }
                }
            });
        });
    }

    // Auto-sync phone data when a device is selected
    use_effect(move || {
        if let Some(ref dev) = *selected_device.read() {
            set_phone_data_device(Some(dev.id.clone()));
            let dev_ip = dev.ip.clone();
            let dev_port = dev.port;

            if *auto_sync_messages.read() && !is_messages_synced() {
                action_tx.send(AppAction::RequestConversationsSync {
                    ip: dev_ip.clone(),
                    port: dev_port,
                });
            }
            if *auto_sync_calls.read() && !is_calls_synced() {
                action_tx.send(AppAction::RequestCallLog {
                    ip: dev_ip.clone(),
                    port: dev_port,
                    limit: 200,
                });
            }
            if *auto_sync_contacts.read() && !is_contacts_synced() {
                action_tx.send(AppAction::RequestContactsSync {
                    ip: dev_ip,
                    port: dev_port,
                });
            }
        } else {
            set_phone_data_device(None);
        }
    });

    // UI Poller - polls state and updates UI signals
    // Uses 500ms interval (increased from 200ms) to reduce CPU usage
    use_future(move || async move {
        let mut last_devices_hash: u64 = 0;
        let mut last_transfer_status_hash: u64 = 0;

        loop {
            // Update devices list (with change detection)
            let mut list: Vec<DeviceInfo> = get_devices_store()
                .lock_or_recover()
                .values()
                .cloned()
                .collect();

            // Apply pending state
            {
                let pending = get_pending_pairings().lock_or_recover();
                for device in list.iter_mut() {
                    if pending.contains(&device.id) {
                        device.is_pending = true;
                    }
                }
            }

            // Only update if devices changed
            let new_devices_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                list.len().hash(&mut hasher);
                for d in &list {
                    d.id.hash(&mut hasher);
                    d.name.hash(&mut hasher);
                    d.ip.hash(&mut hasher);
                    d.port.hash(&mut hasher);
                    d.is_trusted.hash(&mut hasher);
                    d.is_pending.hash(&mut hasher);
                }
                hasher.finish()
            };
            if new_devices_hash != last_devices_hash {
                last_devices_hash = new_devices_hash;
                devices_list.set(list);
            }

            // Update transfer status (with change detection)
            let status = get_transfer_status().lock_or_recover().clone();
            let new_status_hash = match &status {
                TransferStatus::Idle => 0,
                TransferStatus::Compressing {
                    bytes_processed,
                    total_bytes,
                    ..
                } => {
                    // Use progress percentage for hash to detect changes
                    // Use tenths of a percent for smoother updates
                    let percent_x10 = if *total_bytes > 0 {
                        (*bytes_processed).saturating_mul(1000) / *total_bytes
                    } else {
                        0
                    };
                    10000 + percent_x10
                }
                TransferStatus::Starting { .. } => 1,
                TransferStatus::InProgress { percent, .. } => {
                    // Multiply by 10 to get tenths of a percent for finer granularity
                    2 + ((*percent * 10.0) as u64)
                }
                TransferStatus::Completed { .. } => 1000,
                TransferStatus::Failed { .. } => 1001,
                TransferStatus::Cancelled { .. } => 1002,
            };
            if new_status_hash != last_transfer_status_hash {
                last_transfer_status_hash = new_status_hash;
                transfer_status.set(status);
            }

            // Update notifications
            {
                let mut notifs = get_notifications().lock_or_recover();
                let now = std::time::Instant::now();
                notifs.retain(|n| now.duration_since(n.timestamp).as_secs() < 5);
            }
            notifications.set(get_notifications().lock_or_recover().clone());

            // Update Pairing Requests
            {
                let reqs = get_pairing_requests().lock_or_recover().clone();
                pairing_requests.set(reqs);
            }

            // Update File Transfer Requests
            {
                // Cleanup old requests to prevent unbounded growth
                cleanup_old_transfer_requests();

                let reqs_map = get_file_transfer_requests().lock_or_recover();
                let mut reqs: Vec<FileTransferRequest> = reqs_map.values().cloned().collect();
                reqs.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                file_transfer_requests.set(reqs);
            }

            // Update Telephony State
            phone_contacts.set(get_phone_contacts().lock_or_recover().clone());
            phone_conversations.set(get_phone_conversations().lock_or_recover().clone());
            phone_call_log.set(get_phone_call_log().lock_or_recover().clone());
            active_call.set(get_active_call().lock_or_recover().clone());
            // Update messages for selected conversation
            if let Some(thread_id) = selected_conversation.read().clone()
                && let Some(msgs) = get_phone_messages().lock_or_recover().get(&thread_id)
            {
                let new_count = msgs.len();
                let old_count = *last_message_count.read();
                phone_messages.set(msgs.clone());
                // Auto-scroll when messages first load or when new messages arrive
                if new_count > 0 && new_count != old_count {
                    spawn(async move {
                        // Use MutationObserver to wait for DOM update instead of fixed delay
                        // This is more reliable across different machines and message counts
                        let js = r#"
                            (function() {
                                let el = document.getElementById('messages-container');
                                if (!el) return;

                                // First, scroll immediately in case DOM is already updated
                                el.scrollTop = el.scrollHeight;

                                // Then set up observer for any pending updates
                                let observer = new MutationObserver(function(mutations) {
                                    el.scrollTop = el.scrollHeight;
                                    // Disconnect after first mutation to avoid infinite loops
                                    observer.disconnect();
                                });

                                observer.observe(el, { childList: true, subtree: true });

                                // Fallback: disconnect observer after 500ms if no mutations
                                setTimeout(function() {
                                    observer.disconnect();
                                }, 500);
                            })();
                        "#;
                        let _ = document::eval(js);
                    });
                }
                last_message_count.set(new_count);
            }

            // Update Media State
            media_enabled.set(*get_media_enabled().lock_or_recover());
            if let Some(media) = get_current_media().lock_or_recover().clone() {
                current_media.set(CurrentMediaUi {
                    title: media
                        .state
                        .title
                        .unwrap_or_else(|| "Unknown Title".to_string()),
                    artist: media
                        .state
                        .artist
                        .unwrap_or_else(|| "Unknown Artist".to_string()),
                    playing: media.state.playing,
                    source_device_id: media.source_device_id,
                });
            } else {
                current_media.set(CurrentMediaUi::default());
            }

            pairing_mode.set(*get_pairing_mode_state().lock_or_recover());

            // Update Info
            {
                let info = get_update_info().lock_or_recover().clone();
                update_info.set(info);
            }

            // Increased from 200ms to 500ms to reduce CPU usage
            // Most state changes are event-driven, polling is just for sync
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    let toggle_clipboard_sync = move |_| {
        let current = *clipboard_sync_enabled.read();
        let new_state = !current;
        clipboard_sync_enabled.set(new_state);
        set_clipboard_sync_enabled(new_state); // Save to disk
        action_tx.send(AppAction::SetClipboardSync(new_state));
        if new_state {
            *get_last_clipboard().lock_or_recover() = get_system_clipboard();
            add_notification("Clipboard Sync", "Sync Started (Trusted Devices)", "");
        } else {
            add_notification("Clipboard Sync", "Sync Stopped", "");
        }
    };

    let toggle_pairing_mode = move |_| {
        let current = *pairing_mode.read();
        action_tx.send(AppAction::SetPairingMode(!current));
    };

    let toggle_media = move |_| {
        let current = *media_enabled.read();
        let new_state = !current;
        set_media_enabled_setting(new_state); // Save to disk
        action_tx.send(AppAction::ToggleMediaControl {
            enabled: new_state,
            notify: true,
        });
    };

    let app_theme_class = match theme_mode.read().clone() {
        ThemeModeSetting::System => "app-container theme-system",
        ThemeModeSetting::Light => "app-container theme-light",
        ThemeModeSetting::Dark => "app-container theme-dark",
    };

    rsx! {
        style { {include_str!("../assets/styles.css")} }

        div {
            class: "{app_theme_class}",

            // Sidebar
            aside {
                class: "sidebar",

                // Logo/Header
                div {
                    class: "sidebar-header",
                    div {
                        class: "app-icon-surface sidebar-logo",
                        Icon { icon: IconType::Logo, size: 24, color: "currentColor".to_string() }
                    }
                    h1 { "Connected" }
                }

                // Local device info
                div {
                    class: "local-device",
                    div {
                        class: "app-icon-surface local-device-logo",
                        Icon { icon: local_device_icon.clone(), size: 18, color: "currentColor".to_string() }
                    }
                    div {
                        class: "local-device-info",
                        span { class: "local-device-name", "{local_device_name}" }
                        span { class: "local-device-ip", "{local_device_ip}" }
                    }
                    div {
                        class: if is_sdk_initialized() { "status-dot online" } else { "status-dot" }
                    }
                }

                // Navigation
                nav {
                    class: "sidebar-nav",
                    button {
                        class: if *active_tab.read() == "devices" && selected_device.read().is_none() { "nav-item active" } else { "nav-item" },
                        onclick: move |_| {
                            active_tab.set("devices".to_string());
                            selected_device.set(None);
                        },
                        Icon { icon: IconType::NavDevices, size: 18, color: "currentColor".to_string() }
                        span { "Devices" }
                        if !devices_list.read().is_empty() {
                            span { class: "nav-badge", "{devices_list.read().len()}" }
                        }
                    }

                    // Show device feature tabs when a device is selected
                    if selected_device.read().is_some() {
                        div { class: "nav-divider" }
                        if let Some(ref dev) = *selected_device.read() {
                            div {
                                class: "nav-device-name",
                                "{dev.name}"
                            }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "clipboard" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("clipboard".to_string());
                            },
                            Icon { icon: IconType::NavClipboard, size: 18, color: "currentColor".to_string() }
                            span { "Clipboard" }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "transfers" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("transfers".to_string());
                            },
                            Icon { icon: IconType::NavTransfers, size: 18, color: "currentColor".to_string() }
                            span { "Transfers" }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "files" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("files".to_string());
                            },
                            Icon { icon: IconType::NavFiles, size: 18, color: "currentColor".to_string() }
                            span { "Files" }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "media" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("media".to_string());
                            },
                            Icon { icon: IconType::NavMedia, size: 18, color: "currentColor".to_string() }
                            span { "Media" }
                        }
                        button {
                            class: if *active_tab.read() == "devices" && *device_detail_tab.read() == "phone" { "nav-item active" } else { "nav-item" },
                            onclick: move |_| {
                                active_tab.set("devices".to_string());
                                device_detail_tab.set("phone".to_string());
                                // Trigger auto-sync for the current phone sub-tab if enabled and data is empty
                                if let Some(ref dev) = *selected_device.read() {
                                    let current_sub = phone_sub_tab.read().clone();
                                    if current_sub == "messages" && *auto_sync_messages.read() && phone_conversations.read().is_empty() {
                                        action_tx.send(AppAction::RequestConversationsSync {
                                            ip: dev.ip.clone(),
                                            port: dev.port,
                                        });
                                    } else if current_sub == "calls" && *auto_sync_calls.read() && phone_call_log.read().is_empty() {
                                        action_tx.send(AppAction::RequestCallLog {
                                            ip: dev.ip.clone(),
                                            port: dev.port,
                                            limit: 200,
                                        });
                                    } else if current_sub == "contacts" && *auto_sync_contacts.read() && phone_contacts.read().is_empty() {
                                        action_tx.send(AppAction::RequestContactsSync {
                                            ip: dev.ip.clone(),
                                            port: dev.port,
                                        });
                                    }
                                }
                            },
                            Icon { icon: IconType::NavPhone, size: 18, color: "currentColor".to_string() }
                            span { "Phone" }
                        }
                        div { class: "nav-divider" }
                    }

                    button {
                        class: if *active_tab.read() == "settings" { "nav-item active" } else { "nav-item" },
                        onclick: move |_| active_tab.set("settings".to_string()),
                        Icon { icon: IconType::NavSettings, size: 18, color: "currentColor".to_string() }
                        span { "Settings" }
                    }
                }

                // Status indicator
                div {
                    class: "sidebar-footer",
                    div {
                        class: "discovery-status",
                        if is_sdk_initialized() && is_discovery_active() {
                            span { "Discovering" }
                        } else {
                            span { "Starting..." }
                        }
                    }
                }
            }

            // Main content
            main {
                class: "main-content",

                // Tab: Devices
                if *active_tab.read() == "devices" && selected_device.read().is_none() {
                    div {
                        class: "content-header",
                        div {
                            style: "display: flex; align-items: center; gap: 12px;",
                            h2 { "Nearby Devices" }
                            button {
                                class: "header-action-btn",
                                title: "Refresh discovery",
                                onclick: move |_| {
                                    action_tx.send(AppAction::RefreshDevices);
                                },
                                Icon { icon: IconType::Refresh, size: 18, color: "currentColor".to_string() }
                            }
                        }
                        span { class: "content-subtitle", "{devices_list.read().len()} device(s) found" }
                    }

                    if devices_list.read().is_empty() {
                        div {
                            class: "empty-state",
                            div {
                                class: "empty-icon searching",
                                Icon { icon: IconType::Searching, size: 64, color: "var(--text-tertiary)".to_string() }
                            }
                            h3 { "Looking for Devices" }
                            p { "Make sure other devices are on the same network." }
                            div { class: "searching-indicator",
                                span { class: "dot" }
                                span { class: "dot" }
                                span { class: "dot" }
                            }
                        }
                    } else {
                        div {
                            class: "device-grid",
                            for device in devices_list.read().iter() {
                                DeviceCard {
                                    key: "{device.id}",
                                    device: device.clone(),
                                    is_selected: false,
                                    on_select: move |d: DeviceInfo| {
                                        selected_device.set(Some(d));
                                        device_detail_tab.set("clipboard".to_string());
                                    },
                                    on_pair: move |d: DeviceInfo| {
                                        action_tx.send(AppAction::PairWithDevice { ip: d.ip.clone(), port: d.port });
                                    },
                                    on_send_file: move |d: DeviceInfo| {
                                        send_target_device.set(Some(d));
                                        show_send_dialog.set(true);
                                    },
                                }
                            }
                        }
                    }
                }

                // Device Detail View - shows when a device is selected
                if *active_tab.read() == "devices" && selected_device.read().is_some() {
                    if let Some(device) = selected_device.read().as_ref() {
                        div {
                            class: "device-detail-view",

                            // Compact header with device info and actions
                            div {
                                class: "device-detail-header",
                                button {
                                    class: "back-button",
                                    onclick: move |_| {
                                        selected_device.set(None);
                                        active_tab.set("devices".to_string());
                                    },
                                    Icon { icon: IconType::Back, size: 14, color: "currentColor".to_string() }
                                }
                                div {
                                    class: "device-detail-info",
                                    span { class: "device-address", "{device.ip}:{device.port}" }
                                    span { class: "device-type-badge", "{device.device_type}" }
                                }
                                div {
                                    class: "device-detail-actions",
                                    button {
                                        class: "header-action-btn",
                                        title: "Unpair device",
                                        onclick: {
                                            let device = device.clone();
                                            move |_| {
                                                action_tx.send(AppAction::UnpairDevice { device_id: device.id.clone() });
                                                selected_device.set(None);
                                            }
                                        },
                                        Icon { icon: IconType::Unpair, size: 14, color: "currentColor".to_string() }
                                    }
                                    button {
                                        class: "header-action-btn warning",
                                        title: "Forget device",
                                        onclick: {
                                            let device = device.clone();
                                            move |_| {
                                                action_tx.send(AppAction::ForgetDevice { device_id: device.id.clone() });
                                                selected_device.set(None);
                                            }
                                        },
                                        Icon { icon: IconType::Refresh, size: 14, color: "currentColor".to_string() }
                                    }
                                }
                            }

                            // Full-width content area (no secondary sidebar)
                            div {
                                class: "device-detail-content",

                                // Clipboard sub-tab
                                if *device_detail_tab.read() == "clipboard" {
                                    div {
                                        class: "clipboard-section",
                                        div {
                                            class: if *clipboard_sync_enabled.read() { "sync-status active" } else { "sync-status" },
                                            if *clipboard_sync_enabled.read() {
                                                div {
                                                    class: "sync-icon",
                                                    Icon { icon: IconType::Sync, size: 32, color: "var(--accent)".to_string() }
                                                }
                                            } else {
                                                div {
                                                    class: "sync-icon muted",
                                                    Icon { icon: IconType::NavClipboard, size: 32, color: "var(--text-tertiary)".to_string() }
                                                }
                                            }

                                            div {
                                                class: "sync-info",
                                                span {
                                                    class: if *clipboard_sync_enabled.read() { "sync-label" } else { "sync-label muted" },
                                                    "Universal Clipboard"
                                                }
                                                span { class: "sync-hint", "Sync with all trusted devices" }
                                            }

                                            label {
                                                class: "toggle-switch",
                                                input {
                                                    r#type: "checkbox",
                                                    checked: "{clipboard_sync_enabled}",
                                                    oninput: toggle_clipboard_sync,
                                                }
                                                span { class: "slider" }
                                            }
                                        }

                                        div {
                                            class: "manual-clipboard",
                                            h3 { "Send to {device.name}" }
                                            textarea {
                                                class: "clipboard-input",
                                                placeholder: "Enter text to send...",
                                                value: "{clipboard_text}",
                                                oninput: move |evt| clipboard_text.set(evt.value().clone()),
                                            }
                                            div {
                                                class: "clipboard-actions",
                                                button {
                                                    class: "secondary-button",
                                                    onclick: move |_| clipboard_text.set(get_system_clipboard()),
                                                    Icon { icon: IconType::Paste, size: 14, color: "currentColor".to_string() }
                                                    span { " Paste from Clipboard" }
                                                }
                                                button {
                                                    class: "primary-button",
                                                    disabled: clipboard_text.read().is_empty(),
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            let ip = device.ip.clone();
                                                            let port = device.port;
                                                            let text = clipboard_text.read().clone();
                                                            if !text.is_empty() {
                                                                action_tx.send(AppAction::SendClipboard { ip, port, text });
                                                                add_notification("Clipboard", "Sending...", "send");
                                                            }
                                                        }
                                                    },
                                                    Icon { icon: IconType::Send, size: 14, color: "currentColor".to_string() }
                                                    span { " Send" }
                                                }
                                            }
                                        }
                                    }
                                }

                                // Transfers sub-tab
                                if *device_detail_tab.read() == "transfers" {
                                    div {
                                        class: "transfers-section",
                                        match &*transfer_status.read() {
                                            TransferStatus::Idle => rsx! {
                                                div {
                                                    class: "transfer-idle",
                                                    div {
                                                        class: "transfer-icon",
                                                        Icon { icon: IconType::Folder, size: 48, color: "var(--text-tertiary)".to_string() }
                                                    }
                                                    p { "No active transfers" }
                                                }
                                            },
                                            TransferStatus::Compressing {
                                                filename,
                                                current_file,
                                                files_processed,
                                                total_files,
                                                bytes_processed,
                                                total_bytes,
                                                speed_bytes_per_sec,
                                            } => {
                                                let percent = if *total_bytes > 0 {
                                                    (*bytes_processed as f32 / *total_bytes as f32) * 100.0
                                                } else {
                                                    0.0
                                                };
                                                let eta_secs = if *speed_bytes_per_sec > 0 {
                                                    (*total_bytes - *bytes_processed) / *speed_bytes_per_sec
                                                } else {
                                                    0
                                                };
                                                let eta_str = if eta_secs >= 3600 {
                                                    format!("{}:{:02}:{:02}", eta_secs / 3600, (eta_secs % 3600) / 60, eta_secs % 60)
                                                } else if eta_secs >= 60 {
                                                    format!("{}:{:02}", eta_secs / 60, eta_secs % 60)
                                                } else {
                                                    format!("{}s", eta_secs)
                                                };
                                                let speed_str = if *speed_bytes_per_sec >= 1_073_741_824 {
                                                    format!("{:.1} GB/s", *speed_bytes_per_sec as f64 / 1_073_741_824.0)
                                                } else if *speed_bytes_per_sec >= 1_048_576 {
                                                    format!("{:.1} MB/s", *speed_bytes_per_sec as f64 / 1_048_576.0)
                                                } else if *speed_bytes_per_sec >= 1024 {
                                                    format!("{:.1} KB/s", *speed_bytes_per_sec as f64 / 1024.0)
                                                } else {
                                                    format!("{} B/s", speed_bytes_per_sec)
                                                };
                                                let bytes_str = if *total_bytes >= 1_073_741_824 {
                                                    format!("{:.1} GB / {:.1} GB",
                                                        *bytes_processed as f64 / 1_073_741_824.0,
                                                        *total_bytes as f64 / 1_073_741_824.0)
                                                } else if *total_bytes >= 1_048_576 {
                                                    format!("{:.1} MB / {:.1} MB",
                                                        *bytes_processed as f64 / 1_048_576.0,
                                                        *total_bytes as f64 / 1_048_576.0)
                                                } else {
                                                    format!("{:.1} KB / {:.1} KB",
                                                        *bytes_processed as f64 / 1024.0,
                                                        *total_bytes as f64 / 1024.0)
                                                };
                                                rsx! {
                                                    div {
                                                        class: "transfer-active compression-progress",
                                                        div {
                                                            class: "compression-header",
                                                            div {
                                                                class: "transfer-icon spinning",
                                                                Icon { icon: IconType::Sync, size: 32, color: "var(--accent)".to_string() }
                                                            }
                                                            div {
                                                                class: "compression-title",
                                                                h4 { "Compressing folder..." }
                                                                p { class: "compression-filename", "{filename}" }
                                                            }
                                                        }
                                                        div {
                                                            class: "compression-current-file",
                                                            span { "{current_file}" }
                                                        }
                                                        div {
                                                            class: "progress-bar",
                                                            div {
                                                                class: "progress-fill",
                                                                style: "width: {percent}%",
                                                            }
                                                        }
                                                        div {
                                                            class: "compression-stats",
                                                            div {
                                                                class: "stat-row",
                                                                span { class: "stat-label", "{files_processed}/{total_files} files" }
                                                                span { class: "stat-value", "{bytes_str}" }
                                                            }
                                                            div {
                                                                class: "stat-row",
                                                                span { class: "stat-label speed", "{speed_str}" }
                                                                if *speed_bytes_per_sec > 0 {
                                                                    span { class: "stat-value eta", "~{eta_str} remaining" }
                                                                }
                                                            }
                                                        }
                                                        button {
                                                            class: "cancel-button",
                                                            onclick: move |_| {
                                                                action_tx.send(AppAction::CancelFileTransfer);
                                                            },
                                                            Icon { icon: IconType::Close, size: 16, color: "var(--error)".to_string() }
                                                            span { " Cancel Compression" }
                                                        }
                                                    }
                                                }
                                            },
                                            TransferStatus::Starting { filename } => rsx! {
                                                div {
                                                    class: "transfer-active",
                                                    div {
                                                        class: "transfer-icon spinning",
                                                        Icon { icon: IconType::Sync, size: 48, color: "var(--accent)".to_string() }
                                                    }
                                                    p { "Starting transfer: {filename}" }
                                                    button {
                                                        class: "cancel-button",
                                                        onclick: move |_| {
                                                            action_tx.send(AppAction::CancelFileTransfer);
                                                        },
                                                        Icon { icon: IconType::Close, size: 16, color: "var(--error)".to_string() }
                                                        span { " Cancel Transfer" }
                                                    }
                                                }
                                            },
                                            TransferStatus::InProgress { filename, percent } => rsx! {
                                                div {
                                                    class: "transfer-active",
                                                    div { class: "transfer-info",
                                                        span { class: "transfer-filename", "{filename}" }
                                                        span { class: "transfer-percent", "{percent:.1}%" }
                                                    }
                                                    div {
                                                        class: "progress-bar",
                                                        div {
                                                            class: "progress-fill",
                                                            style: "width: {percent}%",
                                                        }
                                                    }
                                                    button {
                                                        class: "cancel-button",
                                                        onclick: move |_| {
                                                            action_tx.send(AppAction::CancelFileTransfer);
                                                        },
                                                        Icon { icon: IconType::Close, size: 16, color: "var(--error)".to_string() }
                                                        span { " Cancel Transfer" }
                                                    }
                                                }
                                            },
                                            TransferStatus::Completed { filename } => rsx! {
                                                div {
                                                    class: "transfer-complete",
                                                    div {
                                                        class: "transfer-icon",
                                                        Icon { icon: IconType::Check, size: 48, color: "var(--success)".to_string() }
                                                    }
                                                    p { "{filename} received successfully!" }
                                                }
                                            },
                                            TransferStatus::Failed { error } => rsx! {
                                                div {
                                                    class: "transfer-failed",
                                                    div {
                                                        class: "transfer-icon",
                                                        Icon { icon: IconType::Error, size: 48, color: "var(--error)".to_string() }
                                                    }
                                                    p { "Transfer failed: {error}" }
                                                }
                                            },
                                            TransferStatus::Cancelled { filename } => rsx! {
                                                div {
                                                    class: "transfer-cancelled",
                                                    div {
                                                        class: "transfer-icon",
                                                        Icon { icon: IconType::Close, size: 48, color: "var(--warning)".to_string() }
                                                    }
                                                    p { "Transfer cancelled: {filename}" }
                                                }
                                            },
                                        }

                                        div {
                                            class: "send-file-section",
                                            h3 { "Send a File or Folder to {device.name}" }
                                            div {
                                                style: "display: flex; gap: 10px;",
                                                button {
                                                    class: "primary-button",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            send_target_device.set(Some(device.clone()));
                                                            send_dialog_is_folder.set(false);
                                                            show_send_dialog.set(true);
                                                        }
                                                    },
                                                    Icon { icon: IconType::Upload, size: 14, color: "currentColor".to_string() }
                                                    span { " Choose File" }
                                                }
                                                button {
                                                    class: "primary-button",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            send_target_device.set(Some(device.clone()));
                                                            send_dialog_is_folder.set(true);
                                                            show_send_dialog.set(true);
                                                        }
                                                    },
                                                    Icon { icon: IconType::Folder, size: 14, color: "currentColor".to_string() }
                                                    span { " Choose Folder" }
                                                }
                                            }
                                        }

                                        div {
                                            class: "download-location",
                                            span { class: "label", "Save to:" }
                                            span {
                                                class: "path",
                                                {get_download_directory_setting()
                                                    .unwrap_or_else(|| dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")).display().to_string())}
                                            }
                                        }
                                    }
                                }

                                // Media sub-tab - Control Android device media from desktop
                                if *device_detail_tab.read() == "media" {
                                    div {
                                        class: "media-section",

                                        // Unified Android Media Control Card
                                        div {
                                            class: "media-controls",
                                            h3 {
                                                Icon { icon: IconType::Music, size: 20, color: "var(--accent)".to_string() }
                                                span { " Control {device.name}" }
                                            }

                                            // Show what's playing on the Android device
                                            div {
                                                class: "media-info",
                                                if current_media.read().title == "Not Playing" || current_media.read().source_device_id != device.id {
                                                    div { class: "muted", "No media playing on this device" }
                                                } else {
                                                    div {
                                                        div { class: "media-title", "{current_media.read().title}" }
                                                        div { class: "media-artist", "{current_media.read().artist}" }
                                                        div {
                                                            class: "media-status",
                                                            if current_media.read().playing {
                                                                Icon { icon: IconType::Play, size: 12, color: "var(--success)".to_string() }
                                                                span { " Playing" }
                                                            } else {
                                                                Icon { icon: IconType::Pause, size: 12, color: "var(--text-secondary)".to_string() }
                                                                span { " Paused" }
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            // Playback Controls
                                            div {
                                                class: "control-buttons",
                                                button {
                                                    class: "control-btn",
                                                    title: "Previous",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port: device.port, command: MediaCommand::Previous });
                                                        }
                                                    },
                                                    Icon { icon: IconType::Previous, size: 24, color: "currentColor".to_string() }
                                                }
                                                button {
                                                    class: "control-btn",
                                                    title: "Play/Pause",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port: device.port, command: MediaCommand::PlayPause });
                                                        }
                                                    },
                                                    Icon { icon: IconType::Play, size: 24, color: "currentColor".to_string() }
                                                }
                                                button {
                                                    class: "control-btn",
                                                    title: "Next",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port: device.port, command: MediaCommand::Next });
                                                        }
                                                    },
                                                    Icon { icon: IconType::Next, size: 24, color: "currentColor".to_string() }
                                                }
                                            }

                                            // Volume Controls
                                            div {
                                                class: "control-buttons secondary",
                                                button {
                                                    class: "control-btn small",
                                                    title: "Volume Down",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port: device.port, command: MediaCommand::VolumeDown });
                                                        }
                                                    },
                                                    Icon { icon: IconType::VolumeDown, size: 20, color: "currentColor".to_string() }
                                                }
                                                button {
                                                    class: "control-btn small",
                                                    title: "Volume Up",
                                                    onclick: {
                                                        let device = device.clone();
                                                        move |_| {
                                                            action_tx.send(AppAction::SendMediaCommand { ip: device.ip.clone(), port: device.port, command: MediaCommand::VolumeUp });
                                                        }
                                                    },
                                                    Icon { icon: IconType::VolumeUp, size: 20, color: "currentColor".to_string() }
                                                }
                                            }
                                        }

                                        // Note about media control
                                        div {
                                            class: "phone-note",
                                            span { class: "note-icon", Icon { icon: IconType::Warning, size: 16, color: "var(--text-tertiary)".to_string() } }
                                            span { "Control media playback on your Android device. Make sure a media app is playing on the device." }
                                        }
                                    }
                                }

                                // Browse Files sub-tab
                                if *device_detail_tab.read() == "files" {
                                    FileBrowser {
                                        device: device.clone(),
                                        on_close: move |_| device_detail_tab.set("clipboard".to_string()),
                                    }
                                }

                                // Phone sub-tab (SMS, Calls, Contacts)
                                if *device_detail_tab.read() == "phone" {
                                    div {
                                        class: "phone-section",

                                        // Phone sub-navigation tabs
                                        div {
                                            class: "phone-tabs",
                                            button {
                                                class: if *phone_sub_tab.read() == "messages" { "phone-tab active" } else { "phone-tab" },
                                                onclick: {
                                                    let device_id = device.id.clone();
                                                    move |_| {
                                                        phone_sub_tab.set("messages".to_string());
                                                        // Only sync if auto-sync enabled AND data is empty
                                                        if *auto_sync_messages.read() && phone_conversations.read().is_empty()
                                                            && let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned()
                                                        {
                                                            action_tx.send(AppAction::RequestConversationsSync {
                                                                ip: fresh_device.ip.clone(),
                                                                port: fresh_device.port,
                                                            });
                                                        }
                                                    }
                                                },
                                                Icon { icon: IconType::Message, size: 16, color: "currentColor".to_string() }
                                                span { " Messages" }
                                            }
                                            button {
                                                class: if *phone_sub_tab.read() == "calls" { "phone-tab active" } else { "phone-tab" },
                                                onclick: {
                                                    let device_id = device.id.clone();
                                                    move |_| {
                                                        phone_sub_tab.set("calls".to_string());
                                                        // Only sync if auto-sync enabled AND data is empty
                                                        if *auto_sync_calls.read() && phone_call_log.read().is_empty()
                                                            && let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned()
                                                        {
                                                            action_tx.send(AppAction::RequestCallLog {
                                                                ip: fresh_device.ip.clone(),
                                                                port: fresh_device.port,
                                                                limit: 200,
                                                            });
                                                        }
                                                    }
                                                },
                                                Icon { icon: IconType::Call, size: 16, color: "currentColor".to_string() }
                                                span { " Calls" }
                                            }
                                            button {
                                                class: if *phone_sub_tab.read() == "contacts" { "phone-tab active" } else { "phone-tab" },
                                                onclick: {
                                                    let device_id = device.id.clone();
                                                    move |_| {
                                                        phone_sub_tab.set("contacts".to_string());
                                                        // Only sync if auto-sync enabled AND data is empty
                                                        if *auto_sync_contacts.read() && phone_contacts.read().is_empty()
                                                            && let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned()
                                                        {
                                                            action_tx.send(AppAction::RequestContactsSync {
                                                                ip: fresh_device.ip.clone(),
                                                                port: fresh_device.port,
                                                            });
                                                        }
                                                    }
                                                },
                                                Icon { icon: IconType::Contact, size: 16, color: "currentColor".to_string() }
                                                span { " Contacts" }
                                            }
                                        }

                                        // Messages Tab Content
                                        if *phone_sub_tab.read() == "messages" {
                                            if let Some(ref thread_id) = *selected_conversation.read() {
                                                // Thread view - show individual messages
                                                {
                                                    let convo_info = phone_conversations.read().iter()
                                                        .find(|c| c.id == *thread_id)
                                                        .cloned();
                                                    let contact_name = convo_info.as_ref()
                                                        .and_then(|c| c.contact_names.first().cloned())
                                                        .or_else(|| convo_info.as_ref().and_then(|c| c.addresses.first().cloned()))
                                                        .unwrap_or_else(|| "Unknown".to_string());
                                                    let recipient_address = convo_info.as_ref()
                                                        .and_then(|c| c.addresses.first().cloned())
                                                        .unwrap_or_default();
                                                    let first_char = contact_name.chars().next().unwrap_or('?');

                                                    rsx! {
                                                        div {
                                                            class: "phone-content message-thread-view",

                                                            // Thread header with back button
                                                            div {
                                                                class: "thread-header",
                                                                button {
                                                                    class: "back-button",
                                                                    onclick: move |_| {
                                                                        selected_conversation.set(None);
                                                                        sms_compose_text.set(String::new());
                                                                        last_message_count.set(0);
                                                                        phone_messages.set(Vec::new());
                                                                    },
                                                                    Icon { icon: IconType::Back, size: 14, color: "currentColor".to_string() }
                                                                    span { " Back" }
                                                                }
                                                                div {
                                                                    class: "thread-contact",
                                                                    div {
                                                                        class: "thread-avatar",
                                                                        "{first_char}"
                                                                    }
                                                                    div {
                                                                        class: "thread-contact-info",
                                                                        span { class: "thread-contact-name", "{contact_name}" }
                                                                        span { class: "thread-contact-number", "{recipient_address}" }
                                                                    }
                                                                }
                                                            }

                                                            // Messages list
                                                            div {
                                                                id: "messages-container",
                                                                class: "messages-container",
                                                                if phone_messages.read().is_empty() {
                                                                    div {
                                                                        class: "empty-state",
                                                                        span { class: "empty-icon", Icon { icon: IconType::Message, size: 48, color: "var(--text-tertiary)".to_string() } }
                                                                        p { "Loading messages..." }
                                                                    }
                                                                } else {
                                                                    div {
                                                                        class: "messages-list",
                                                                        for msg in phone_messages.read().iter() {
                                                                            {
                                                                                let is_outgoing = msg.is_outgoing;
                                                                                let body = msg.body.clone();
                                                                                let time_str = format_timestamp(msg.timestamp);
                                                                                let preview_image = attachment_preview_image(&msg.attachments);
                                                                                let status_icon = match msg.status {
                                                                                    connected_core::telephony::SmsStatus::Pending => Some(IconType::Searching),
                                                                                    connected_core::telephony::SmsStatus::Sent => Some(IconType::Check),
                                                                                    connected_core::telephony::SmsStatus::Delivered => Some(IconType::Check),
                                                                                    connected_core::telephony::SmsStatus::Failed => Some(IconType::Error),
                                                                                    connected_core::telephony::SmsStatus::Received => None,
                                                                                };
                                                                                rsx! {
                                                                                    div {
                                                                                        class: if is_outgoing { "message-bubble outgoing" } else { "message-bubble incoming" },
                                                                                        if !body.is_empty() {
                                                                                            div { class: "message-body", "{body}" }
                                                                                        }
                                                                                        if let Some(image) = preview_image {
                                                                                            div {
                                                                                                class: "message-image-wrap",
                                                                                                img {
                                                                                                    class: "message-image",
                                                                                                    src: "{image.data_url}",
                                                                                                    alt: "MMS image attachment",
                                                                                                    onclick: {
                                                                                                        let image = image.clone();
                                                                                                        move |_| {
                                                                                                            open_mms_image.set(Some(image.clone()));
                                                                                                        }
                                                                                                    }
                                                                                                }
                                                                                            }
                                                                                        }
                                                                                        div {
                                                                                            class: "message-meta",
                                                                                            span { class: "message-time", "{time_str}" }
                                                                                            if is_outgoing {
                                                                                                if let Some(icon) = status_icon {
                                                                                                    span { class: "message-status", Icon { icon: icon, size: 12, color: "currentColor".to_string() } }
                                                                                                }
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }

                                                            // Compose area
                                                            div {
                                                                class: "compose-area",
                                                                input {
                                                                    r#type: "text",
                                                                    class: "compose-input",
                                                                    placeholder: "Type a message...",
                                                                    value: "{sms_compose_text.read()}",
                                                                    oninput: move |e| sms_compose_text.set(e.value().clone())
                                                                }
                                                                button {
                                                                    class: "send-button",
                                                                    disabled: sms_compose_text.read().is_empty(),
                                                                    onclick: {
                                                                        let device_id = device.id.clone();
                                                                        let to_address = recipient_address.clone();
                                                                        move |_| {
                                                                            let text = sms_compose_text.read().clone();
                                                                            if !text.is_empty() && !to_address.is_empty()
                                                                                && let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned()
                                                                            {
                                                                                action_tx.send(AppAction::SendSms {
                                                                                    ip: fresh_device.ip.clone(),
                                                                                    port: fresh_device.port,
                                                                                    to: to_address.clone(),
                                                                                    body: text,
                                                                                });
                                                                                sms_compose_text.set(String::new());
                                                                            }
                                                                        }
                                                                    },
                                                                    "Send"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            } else {
                                                // Conversation list view
                                                div {
                                                    class: "phone-content",
                                                    div {
                                                        class: "phone-header",
                                                        h4 { "Messages" }
                                                        if !*auto_sync_messages.read() {
                                                            button {
                                                                class: "sync-button",
                                                                onclick: {
                                                                    let device_id = device.id.clone();
                                                                    move |_| {
                                                                        if let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned() {
                                                                            action_tx.send(AppAction::RequestConversationsSync {
                                                                                ip: fresh_device.ip.clone(),
                                                                                port: fresh_device.port,
                                                                            });
                                                                        } else {
                                                                            add_notification("Phone", "Device not found. Try refreshing.", "");
                                                                        }
                                                                    }
                                                                },
                                                                Icon { icon: IconType::Refresh, size: 14, color: "currentColor".to_string() }
                                                                span { " Sync" }
                                                            }
                                                        }
                                                    }

                                                    if phone_conversations.read().is_empty() {
                                                            div {
                                                                class: "empty-state",
                                                                span { class: "empty-icon", Icon { icon: IconType::Message, size: 48, color: "var(--text-tertiary)".to_string() } }
                                                                p { "No messages synced" }
                                                                if *auto_sync_messages.read() {
                                                                    p { class: "empty-hint", "Messages will sync automatically" }
                                                                } else {
                                                                    p { class: "empty-hint", "Click Sync to load your messages" }
                                                                }
                                                            }
                                                    } else {
                                                        div {
                                                            class: "conversation-list",
                                                            for convo in phone_conversations.read().iter() {
                                                                {
                                                                    let convo_id = convo.id.clone();
                                                                    let contact_name = convo.contact_names.first()
                                                                        .cloned()
                                                                        .unwrap_or_else(|| convo.addresses.first().cloned().unwrap_or_default());
                                                                    let last_msg = convo.last_message.clone().unwrap_or_default();
                                                                    let unread = convo.unread_count;
                                                                    let time_str = format_timestamp(convo.last_timestamp);

                                                                    rsx! {
                                                                        div {
                                                                            class: if unread > 0 { "conversation-item unread" } else { "conversation-item" },
                                                                            onclick: {
                                                                                let cid = convo_id.clone();
                                                                                let device_id = device.id.clone();
                                                                                move |_| {
                                                                                    selected_conversation.set(Some(cid.clone()));
                                                                                    last_message_count.set(0); // Reset to trigger scroll on load
                                                                                    phone_messages.set(Vec::new()); // Clear old messages
                                                                                    if let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned() {
                                                                                        action_tx.send(AppAction::RequestMessages {
                                                                                            ip: fresh_device.ip.clone(),
                                                                                            port: fresh_device.port,
                                                                                            thread_id: cid.clone(),
                                                                                            limit: 200,
                                                                                        });
                                                                                    }
                                                                                }
                                                                            },
                                                                            div {
                                                                                class: "conversation-avatar",
                                                                                "{contact_name.chars().next().unwrap_or('?')}"
                                                                            }
                                                                            div {
                                                                                class: "conversation-info",
                                                                                div {
                                                                                    class: "conversation-header",
                                                                                    span { class: "conversation-name", "{contact_name}" }
                                                                                    span { class: "conversation-time", "{time_str}" }
                                                                                }
                                                                                div {
                                                                                    class: "conversation-preview",
                                                                                    "{last_msg}"
                                                                                }
                                                                            }
                                                                            if unread > 0 {
                                                                                span { class: "unread-badge", "{unread}" }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // Calls Tab Content
                                        if *phone_sub_tab.read() == "calls" {
                                            div {
                                                class: "phone-content",
                                                div {
                                                    class: "phone-header",
                                                    h4 { "Call Log" }
                                                    if !*auto_sync_calls.read() {
                                                        button {
                                                            class: "sync-button",
                                                            onclick: {
                                                                let device_id = device.id.clone();
                                                                move |_| {
                                                                    if let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned() {
                                                                        action_tx.send(AppAction::RequestCallLog {
                                                                            ip: fresh_device.ip.clone(),
                                                                            port: fresh_device.port,
                                                                            limit: 200,
                                                                        });
                                                                    } else {
                                                                        add_notification("Phone", "Device not found. Try refreshing.", "");
                                                                    }
                                                                }
                                                            },
                                                            Icon { icon: IconType::Refresh, size: 14, color: "currentColor".to_string() }
                                                            span { " Sync" }
                                                        }
                                                    }
                                                }

                                                if phone_call_log.read().is_empty() {
                                                    div {
                                                        class: "empty-state",
                                                        span { class: "empty-icon", Icon { icon: IconType::Call, size: 48, color: "var(--text-tertiary)".to_string() } }
                                                        p { "No call history synced" }
                                                        if *auto_sync_calls.read() {
                                                            p { class: "empty-hint", "Call log will sync automatically" }
                                                        } else {
                                                            p { class: "empty-hint", "Click Sync to load your call log" }
                                                        }
                                                    }
                                                } else {
                                                    div {
                                                        class: "call-log-list",
                                                        for call in phone_call_log.read().iter() {
                                                            {
                                                                let contact = call.contact_name.clone().unwrap_or_else(|| call.number.clone());
                                                                let call_type = &call.call_type;
                                                                let duration = call.duration;
                                                                let timestamp = call.timestamp;
                                                                let time_str = format_timestamp(timestamp);
                                                                let (icon_type, type_class) = match call_type {
                                                                    connected_core::telephony::CallType::Incoming => (IconType::Download, "incoming"),
                                                                    connected_core::telephony::CallType::Outgoing => (IconType::Upload, "outgoing"),
                                                                    connected_core::telephony::CallType::Missed => (IconType::Error, "missed"),
                                                                    connected_core::telephony::CallType::Rejected => (IconType::Warning, "rejected"),
                                                                    _ => (IconType::Call, "other"),
                                                                };
                                                                let duration_str = if duration > 0 {
                                                                    format!("{}:{:02}", duration / 60, duration % 60)
                                                                } else {
                                                                    "—".to_string()
                                                                };

                                                                rsx! {
                                                                    div {
                                                                        class: "call-item {type_class}",
                                                                        div { class: "call-icon", Icon { icon: icon_type, size: 16, color: "currentColor".to_string() } }
                                                                        div {
                                                                            class: "call-info",
                                                                            div {
                                                                                class: "call-contact",
                                                                                "{contact}"
                                                                            }
                                                                            div {
                                                                                class: "call-meta",
                                                                                span { class: "call-time", "{time_str}" }
                                                                                span { class: "call-duration", "{duration_str}" }
                                                                            }
                                                                        }
                                                                        button {
                                                                            class: "call-action",
                                                                            onclick: {
                                                                                let number = call.number.clone();
                                                                                let device_id = device.id.clone();
                                                                                move |_| {
                                                                                    if let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned() {
                                                                                        action_tx.send(AppAction::InitiateCall {
                                                                                            ip: fresh_device.ip.clone(),
                                                                                            port: fresh_device.port,
                                                                                            number: number.clone(),
                                                                                        });
                                                                                    }
                                                                                }
                                                                            },
                                                                            Icon { icon: IconType::Call, size: 16, color: "currentColor".to_string() }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        // Contacts Tab Content
                                        if *phone_sub_tab.read() == "contacts" {
                                            div {
                                                class: "phone-content",
                                                div {
                                                    class: "phone-header",
                                                    h4 { "Contacts ({phone_contacts.read().len()})" }
                                                    if !*auto_sync_contacts.read() {
                                                        button {
                                                            class: "sync-button",
                                                            onclick: {
                                                                let device_id = device.id.clone();
                                                                move |_| {
                                                                    if let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned() {
                                                                        action_tx.send(AppAction::RequestContactsSync {
                                                                            ip: fresh_device.ip.clone(),
                                                                            port: fresh_device.port,
                                                                        });
                                                                    } else {
                                                                        add_notification("Phone", "Device not found. Try refreshing.", "");
                                                                    }
                                                                }
                                                            },
                                                            Icon { icon: IconType::Refresh, size: 14, color: "currentColor".to_string() }
                                                            span { " Sync" }
                                                        }
                                                    }
                                                }

                                                if phone_contacts.read().is_empty() {
                                                    div {
                                                        class: "empty-state",
                                                        span { class: "empty-icon", Icon { icon: IconType::Contact, size: 48, color: "var(--text-tertiary)".to_string() } }
                                                        p { "No contacts synced" }
                                                        if *auto_sync_contacts.read() {
                                                            p { class: "empty-hint", "Contacts will sync automatically" }
                                                        } else {
                                                            p { class: "empty-hint", "Click Sync to load your contacts" }
                                                        }
                                                    }
                                                } else {
                                                    div {
                                                        class: "contacts-list",
                                                        for contact in phone_contacts.read().iter() {
                                                            {
                                                                let name = contact.name.clone();
                                                                let phone = contact.phone_numbers.first()
                                                                    .map(|p| p.number.clone())
                                                                    .unwrap_or_default();
                                                                let starred = contact.starred;

                                                                rsx! {
                                                                    div {
                                                                        class: "contact-item",
                                                                        div {
                                                                            class: "contact-avatar",
                                                                            "{name.chars().next().unwrap_or('?')}"
                                                                        }
                                                                        div {
                                                                            class: "contact-info",
                                                                            div {
                                                                                class: "contact-name",
                                                                                if starred {
                                                                                     Icon { icon: IconType::Star, size: 12, color: "var(--accent)".to_string() }
                                                                                }
                                                                                "{name}"
                                                                            }
                                                                            if !phone.is_empty() {
                                                                                div {
                                                                                    class: "contact-phone",
                                                                                    "{phone}"
                                                                                }
                                                                            }
                                                                        }
                                                                        div {
                                                                            class: "contact-actions",
                                                                            if !phone.is_empty() {
                                                                                button {
                                                                                    class: "contact-action",
                                                                                    title: "Call",
                                                                                    onclick: {
                                                                                        let number = phone.clone();
                                                                                        let device_id = device.id.clone();
                                                                                        move |_| {
                                                                                            if let Some(fresh_device) = get_devices_store().lock_or_recover().get(&device_id).cloned() {
                                                                                                action_tx.send(AppAction::InitiateCall {
                                                                                                    ip: fresh_device.ip.clone(),
                                                                                                    port: fresh_device.port,
                                                                                                    number: number.clone(),
                                                                                                });
                                                                                            }
                                                                                        }
                                                                                    },
                                                                                    Icon { icon: IconType::Call, size: 16, color: "currentColor".to_string() }
                                                                                }
                                                                                button {
                                                                                    class: "contact-action",
                                                                                    title: "Message",
                                                                                    Icon { icon: IconType::Message, size: 16, color: "currentColor".to_string() }
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Tab: Settings
                if *active_tab.read() == "settings" {
                    div {
                        class: "settings-section",
                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::Refresh, size: 20, color: "currentColor".to_string() }
                                " Updates"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Current Version" }
                                div { class: "info-value", "{env!(\"CARGO_PKG_VERSION\")}" }

                                if let Some(info) = update_info.read().as_ref() {
                                    div { class: "info-label", "Latest Version" }
                                    div { class: "info-value", "{info.latest_version}" }

                                    if info.has_update {
                                         div { class: "info-label", "Status" }
                                         div {
                                             class: "info-value",
                                             style: "color: var(--accent); font-weight: bold;",
                                             "Update Available"
                                         }
                                    }
                                }
                            }

                            div {
                                class: "settings-actions",
                                style: "margin-top: 15px; display: flex; gap: 10px;",

                                button {
                                    class: "secondary-button",
                                    onclick: move |_| action_tx.send(AppAction::CheckForUpdates),
                                    "Check for Updates"
                                }

                                if let Some(info) = update_info.read().as_ref() {
                                    if info.has_update {
                                         button {
                                            class: "primary-button",
                                            onclick: move |_| action_tx.send(AppAction::PerformUpdate),
                                            "Update Now"
                                        }
                                    }
                                }
                            }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::Desktop, size: 20, color: "currentColor".to_string() }
                                " This Device"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Name" }
                                div {
                                    class: "info-value",
                                    style: "display: flex; justify-content: space-between; align-items: center;",
                                    span { "{local_device_name}" }
                                    button {
                                        class: "secondary-button",
                                        style: "padding: 4px 8px; font-size: 0.8rem; margin-left: 8px;",
                                        onclick: move |_| {
                                            rename_text.set(local_device_name.read().clone());
                                            show_rename_dialog.set(true);
                                        },
                                        "Rename"
                                    }
                                }
                                div { class: "info-label", "IP" }
                                div { class: "info-value", "{local_device_ip}" }
                            }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::NavSettings, size: 20, color: "currentColor".to_string() }
                                " Appearance"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Theme" }
                                div {
                                    class: "info-value",
                                    style: "display: flex; gap: 8px; flex-wrap: wrap;",
                                    button {
                                        class: if matches!(*theme_mode.read(), ThemeModeSetting::System) { "toggle-button active" } else { "toggle-button" },
                                        onclick: move |_| {
                                            let mode = ThemeModeSetting::System;
                                            theme_mode.set(mode.clone());
                                            set_theme_mode_setting(mode);
                                        },
                                        "System"
                                    }
                                    button {
                                        class: if matches!(*theme_mode.read(), ThemeModeSetting::Light) { "toggle-button active" } else { "toggle-button" },
                                        onclick: move |_| {
                                            let mode = ThemeModeSetting::Light;
                                            theme_mode.set(mode.clone());
                                            set_theme_mode_setting(mode);
                                        },
                                        "Light"
                                    }
                                    button {
                                        class: if matches!(*theme_mode.read(), ThemeModeSetting::Dark) { "toggle-button active" } else { "toggle-button" },
                                        onclick: move |_| {
                                            let mode = ThemeModeSetting::Dark;
                                            theme_mode.set(mode.clone());
                                            set_theme_mode_setting(mode);
                                        },
                                        "Dark"
                                    }
                                }
                            }
                            p { class: "settings-hint", "System follows your OS preference; Light and Dark are pinned." }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::Download, size: 20, color: "currentColor".to_string() }
                                " Download Location"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Save files to" }
                                div {
                                    class: "info-value",
                                    style: "display: flex; justify-content: space-between; align-items: center; gap: 8px;",
                                    span {
                                        style: "overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1;",
                                        "{download_directory}"
                                    }
                                    button {
                                        class: "secondary-button",
                                        style: "padding: 4px 8px; font-size: 0.8rem; white-space: nowrap;",
                                        onclick: move |_| {
                                            let current_dir = download_directory.read().clone();
                                            if let Some(path) = rfd::FileDialog::new()
                                                .set_directory(&current_dir)
                                                .pick_folder()
                                            {
                                                let path_str = path.display().to_string();
                                                download_directory.set(path_str.clone());
                                                action_tx.send(AppAction::SetDownloadDirectory { path: path_str });
                                            }
                                        },
                                        "Browse"
                                    }
                                }
                            }
                            p { class: "settings-hint", "Choose where received files and downloads are saved." }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::Pair, size: 20, color: "currentColor".to_string() }
                                " Security"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Pairing Mode" }
                                div {
                                    class: "info-value",
                                    button {
                                        class: if *pairing_mode.read() { "toggle-button active" } else { "toggle-button" },
                                        onclick: toggle_pairing_mode,
                                        if *pairing_mode.read() { "Enabled" } else { "Disabled" }
                                    }
                                }
                            }
                            p { class: "settings-hint", "Enable to allow new devices to pair with you." }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::NavClipboard, size: 20, color: "currentColor".to_string() }
                                " Clipboard Sync"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Auto-sync" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{clipboard_sync_enabled}",
                                            oninput: toggle_clipboard_sync,
                                        }
                                        span { class: "slider" }
                                    }
                                }
                            }
                            p { class: "settings-hint", "Automatically sync clipboard with trusted devices." }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::Music, size: 20, color: "currentColor".to_string() }
                                " Media Sharing"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Share Status" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{media_enabled}",
                                            oninput: toggle_media,
                                        }
                                        span { class: "slider" }
                                    }
                                }
                            }
                            p { class: "settings-hint", "Allow other devices to see and control your media." }
                        }

                        if cfg!(target_os = "linux") || cfg!(target_os = "windows") {
                            div {
                                class: "info-card",
                                h3 {
                                    Icon { icon: IconType::Bell, size: 20, color: "currentColor".to_string() }
                                    " Notifications"
                                }
                                div {
                                    class: "info-grid",
                                    div { class: "info-label", "Enable" }
                                    div {
                                        class: "info-value",
                                        label {
                                            class: "toggle-switch",
                                            input {
                                                r#type: "checkbox",
                                                checked: "{notifications_enabled}",
                                                oninput: move |_| {
                                                    let new_val = !*notifications_enabled.read();
                                                    notifications_enabled.set(new_val);
                                                    set_notifications_enabled_setting(new_val);
                                                },
                                            }
                                            span { class: "slider" }
                                        }
                                    }
                                }
                                p { class: "settings-hint", "Show system notifications on this device." }
                            }
                        }

                        div {
                            class: "info-card",
                            h3 {
                                Icon { icon: IconType::NavPhone, size: 20, color: "currentColor".to_string() }
                                " Phone Auto-Sync"
                            }
                            div {
                                class: "info-grid",
                                div { class: "info-label", "Messages" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{auto_sync_messages}",
                                            oninput: move |_| {
                                                let new_val = !*auto_sync_messages.read();
                                                auto_sync_messages.set(new_val);
                                                set_auto_sync_messages(new_val);
                                            },
                                        }
                                        span { class: "slider" }
                                    }
                                }
                                div { class: "info-label", "Calls" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{auto_sync_calls}",
                                            oninput: move |_| {
                                                let new_val = !*auto_sync_calls.read();
                                                auto_sync_calls.set(new_val);
                                                set_auto_sync_calls(new_val);
                                            },
                                        }
                                        span { class: "slider" }
                                    }
                                }
                                div { class: "info-label", "Contacts" }
                                div {
                                    class: "info-value",
                                    label {
                                        class: "toggle-switch",
                                        input {
                                            r#type: "checkbox",
                                            checked: "{auto_sync_contacts}",
                                            oninput: move |_| {
                                                let new_val = !*auto_sync_contacts.read();
                                                auto_sync_contacts.set(new_val);
                                                set_auto_sync_contacts(new_val);
                                            },
                                        }
                                        span { class: "slider" }
                                    }
                                }
                            }
                            p { class: "settings-hint", "Automatically sync phone data when opening a device. Hides manual sync buttons when enabled." }
                        }
                    }
                }
            }

            // Notifications (in-app)
            if !cfg!(target_os = "linux") && !cfg!(target_os = "windows") {
                div {
                    class: "notifications-panel",
                    for notification in notifications.read().iter().rev().take(3) {
                        div {
                            class: "notification",
                            key: "{notification.id}",
                            span { class: "notification-icon", "{notification.icon}" }
                            div {
                                class: "notification-content",
                                span { class: "notification-title", "{notification.title}" }
                                span { class: "notification-message", "{notification.message}" }
                            }
                        }
                    }
                }
            }

            // MMS Image Viewer Modal
            if let Some(image) = open_mms_image.read().clone() {
                div {
                    class: "modal-overlay mms-image-modal-overlay",
                    onclick: move |_| open_mms_image.set(None),

                    div {
                        class: "modal-content mms-image-modal-content",
                        onclick: move |evt| evt.stop_propagation(),

                        div {
                            class: "mms-image-modal-body",
                            img {
                                class: "mms-image-modal-full",
                                src: "{image.data_url}",
                                alt: "MMS image preview"
                            }
                        }

                        div {
                            class: "modal-actions mms-image-modal-actions",
                            button {
                                class: "primary-button",
                                onclick: {
                                    let image = image.clone();
                                    let configured_dir = download_directory.read().clone();
                                    move |_| {
                                        match save_mms_image_to_downloads(&image, &configured_dir) {
                                            Ok(path) => {
                                                add_notification(
                                                    "Phone",
                                                    &format!("Downloaded image to {}", path.display()),
                                                    "",
                                                );
                                            }
                                            Err(err) => {
                                                add_notification(
                                                    "Download Failed",
                                                    &err,
                                                    "",
                                                );
                                            }
                                        }
                                    }
                                },
                                Icon { icon: IconType::Download, size: 14, color: "currentColor".to_string() }
                                span { "Download" }
                            }
                            button {
                                class: "secondary-button",
                                onclick: move |_| open_mms_image.set(None),
                                "Close"
                            }
                        }
                    }
                }
            }

            // Pairing Requests Modal
            if !pairing_requests.read().is_empty() {
                div {
                    class: "modal-overlay",
                    div {
                        class: "modal-content",
                        h3 { "Pairing Request" }
                        for req in pairing_requests.read().iter() {
                            div {
                                key: "{req.fingerprint}",
                                class: "pairing-request",
                                p { "Device: {req.device_name}" }
                                p { class: "fingerprint", "ID: {req.fingerprint}" }
                                div {
                                    class: "modal-actions",
                                    button {
                                        class: "secondary-button",
                                        onclick: {
                                            let req = req.clone();
                                            move |_| {
                                                action_tx.send(AppAction::RejectDevice {
                                                    fingerprint: req.fingerprint.clone(),
                                                    device_id: req.device_id.clone(),
                                                });
                                                get_pairing_requests().lock_or_recover().retain(|r| r.fingerprint != req.fingerprint);
                                            }
                                        },
                                        "Reject"
                                    }
                                    button {
                                        class: "primary-button",
                                        onclick: {
                                            let req = req.clone();
                                            move |_| {
                                                action_tx.send(AppAction::TrustDevice {
                                                    fingerprint: req.fingerprint.clone(),
                                                    name: req.device_name.clone(),
                                                    device_id: req.device_id.clone()
                                                });
                                                get_pairing_requests().lock_or_recover().retain(|r| r.fingerprint != req.fingerprint);
                                            }
                                        },
                                        "Trust"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // File Transfer Requests Modal
            if !file_transfer_requests.read().is_empty() {
                div {
                    class: "modal-overlay",
                    div {
                        class: "modal-content",
                        h3 { "Incoming File Transfer" }
                        for req in file_transfer_requests.read().iter() {
                            div {
                                key: "{req.id}",
                                class: "pairing-request", // Reuse styling
                                p { "From: {req.from_device}" }
                                p { class: "fingerprint", "File: {req.filename}" }
                                p { class: "fingerprint", "Size: {req.size} bytes" }
                                div {
                                    class: "modal-actions",
                                    button {
                                        class: "secondary-button",
                                        onclick: {
                                            let req = req.clone();
                                            move |_| {
                                                action_tx.send(AppAction::RejectFileTransfer { transfer_id: req.id.clone() });
                                            }
                                        },
                                        "Reject"
                                    }
                                    button {
                                        class: "primary-button",
                                        onclick: {
                                            let req = req.clone();
                                            move |_| {
                                                action_tx.send(AppAction::AcceptFileTransfer { transfer_id: req.id.clone() });
                                            }
                                        },
                                        "Accept"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Incoming/Active Call Modal
            if let Some(call) = active_call.read().clone() {
                div {
                    class: "modal-overlay call-overlay",
                    div {
                        class: "modal-content call-modal",
                        // Call state header
                        div {
                            class: "call-modal-header",
                            match call.state {
                                ActiveCallState::Ringing => rsx! {
                                    div { class: "call-state-icon ringing", Icon { icon: IconType::Call, size: 48, color: "currentColor".to_string() } }
                                    h3 { "Incoming Call" }
                                },
                                ActiveCallState::Dialing => rsx! {
                                    div { class: "call-state-icon dialing", Icon { icon: IconType::NavPhone, size: 48, color: "currentColor".to_string() } }
                                    h3 { "Dialing..." }
                                },
                                ActiveCallState::Connected => rsx! {
                                    div { class: "call-state-icon connected", Icon { icon: IconType::VolumeUp, size: 48, color: "currentColor".to_string() } }
                                    h3 { "Call in Progress" }
                                },
                                ActiveCallState::OnHold => rsx! {
                                    div { class: "call-state-icon on-hold", Icon { icon: IconType::Pause, size: 48, color: "currentColor".to_string() } }
                                    h3 { "On Hold" }
                                },
                                ActiveCallState::Ended => rsx! {
                                    div { class: "call-state-icon ended", Icon { icon: IconType::Unpair, size: 48, color: "currentColor".to_string() } }
                                    h3 { "Call Ended" }
                                },
                            }
                        }

                        // Caller info
                        div {
                            class: "call-caller-info",
                            div {
                                class: "call-avatar",
                                {call.contact_name.as_ref().and_then(|n| n.chars().next()).unwrap_or('?').to_string()}
                            }
                            div {
                                class: "call-caller-details",
                                p {
                                    class: "call-caller-name",
                                    {call.contact_name.clone().unwrap_or_else(|| call.number.clone())}
                                }
                                p {
                                    class: "call-caller-number",
                                    {call.number.clone()}
                                }
                            }
                        }

                        // Call duration for connected calls
                        if call.state == ActiveCallState::Connected && call.duration > 0 {
                            div {
                                class: "call-duration",
                                {format!("{}:{:02}", call.duration / 60, call.duration % 60)}
                            }
                        }

                        // Action buttons
                        div {
                            class: "call-modal-actions",
                            match call.state {
                                ActiveCallState::Ringing => {
                                    // Show Answer and Reject buttons for incoming ringing calls
                                    rsx! {
                                        button {
                                            class: "call-button reject",
                                            onclick: {
                                                let selected = selected_device.read().clone();
                                                move |_| {
                                                    if let Some(ref device) = selected {
                                                        action_tx.send(AppAction::SendCallAction {
                                                            ip: device.ip.clone(),
                                                            port: device.port,
                                                            action: CallAction::Reject,
                                                        });
                                                    }
                                                }
                                            },
                                            span { class: "call-btn-icon", Icon { icon: IconType::Unpair, size: 20, color: "currentColor".to_string() } }
                                            span { "Reject" }
                                        }
                                        button {
                                            class: "call-button answer",
                                            onclick: {
                                                let selected = selected_device.read().clone();
                                                move |_| {
                                                    if let Some(ref device) = selected {
                                                        action_tx.send(AppAction::SendCallAction {
                                                            ip: device.ip.clone(),
                                                            port: device.port,
                                                            action: CallAction::Answer,
                                                        });
                                                    }
                                                }
                                            },
                                            span { class: "call-btn-icon", Icon { icon: IconType::Call, size: 20, color: "currentColor".to_string() } }
                                            span { "Answer" }
                                        }
                                    }
                                },
                                ActiveCallState::Connected | ActiveCallState::OnHold | ActiveCallState::Dialing => {
                                    // Show End Call button for active calls
                                    rsx! {
                                        button {
                                            class: "call-button end-call",
                                            onclick: {
                                                let selected = selected_device.read().clone();
                                                move |_| {
                                                    if let Some(ref device) = selected {
                                                        action_tx.send(AppAction::SendCallAction {
                                                            ip: device.ip.clone(),
                                                            port: device.port,
                                                            action: CallAction::HangUp,
                                                        });
                                                    }
                                                }
                                            },
                                            span { class: "call-btn-icon", Icon { icon: IconType::Unpair, size: 20, color: "currentColor".to_string() } }
                                            span { "End Call" }
                                        }
                                    }
                                },
                                ActiveCallState::Ended => {
                                    // Show dismiss button for ended calls
                                    rsx! {
                                        button {
                                            class: "call-button dismiss",
                                            onclick: move |_| {
                                                set_active_call(None);
                                            },
                                            span { "Dismiss" }
                                        }
                                    }
                                },
                            }
                        }
                    }
                }
            }

            if *show_rename_dialog.read() {
                div {
                    class: "modal-overlay",
                    div {
                        class: "modal-content",
                        h3 { "Rename Device" }
                        div {
                            style: "margin: 16px 0;",
                            input {
                                r#type: "text",
                                style: "width: 100%; padding: 8px; border: 1px solid var(--border-color); border-radius: 4px; background: var(--bg-secondary); color: var(--text-primary);",
                                value: "{rename_text}",
                                oninput: move |e| rename_text.set(e.value().clone()),
                                placeholder: "Enter new name"
                            }
                        }
                        div {
                            class: "modal-actions",
                            button {
                                class: "secondary-button",
                                onclick: move |_| show_rename_dialog.set(false),
                                "Cancel"
                            }
                            button {
                                class: "primary-button",
                                onclick: move |_| {
                                    let new_name = rename_text.read().trim().to_string();
                                    if !new_name.is_empty() {
                                        local_device_name.set(new_name.clone());
                                        action_tx.send(AppAction::RenameDevice { new_name });
                                        show_rename_dialog.set(false);
                                    }
                                },
                                "Save"
                            }
                        }
                    }
                }
            }

            if *show_send_dialog.read() {
                FileDialog {
                    device: send_target_device.read().clone(),
                    is_folder: *send_dialog_is_folder.read(),
                    on_close: move |_| {
                        show_send_dialog.set(false);
                        send_target_device.set(None);
                    },
                    on_send: move |path: String| {
                        let final_path = path.clone();
                        let success = true;

                        if let Some(ref device) = *send_target_device.read()
                            && success
                        {
                            action_tx.send(AppAction::SendFile {
                                ip: device.ip.clone(),
                                port: device.port,
                                path: final_path
                            });
                        }
                        show_send_dialog.set(false);
                        send_target_device.set(None);
                    }
                }
            }
        }
    }
}
