use crate::controller::AppAction;
use crate::state::{
    get_current_remote_files, get_current_remote_path, get_preview_data, get_remote_files_update,
    DeviceInfo, PreviewData,
};
use base64::Engine as _;
use connected_core::filesystem::{FsEntry, FsEntryType};
use dioxus::prelude::*;

#[component]
pub fn FileBrowser(device: DeviceInfo, on_close: EventHandler<()>) -> Element {
    let action_tx = use_coroutine_handle::<AppAction>();
    let mut current_path = use_signal(|| get_current_remote_path().lock().unwrap().clone());
    let mut files = use_signal(|| Option::<Vec<FsEntry>>::None);
    let mut loading = use_signal(|| false);
    let mut last_update_seen = use_signal(|| *get_remote_files_update().lock().unwrap());
    let mut context_menu = use_signal(|| Option::<(String, String, i32, i32)>::None);
    let mut preview_content = use_signal(|| Option::<PreviewData>::None);

    // Initial load
    use_effect(use_reactive(&device, move |device| {
        loading.set(true);
        action_tx.send(AppAction::ListRemoteFiles {
            ip: device.ip.clone(),
            port: device.port,
            path: "/".to_string(),
        });
    }));

    // Poll for updates
    use_future(move || async move {
        loop {
            let global_update = *get_remote_files_update().lock().unwrap();
            let new_files = get_current_remote_files().lock().unwrap().clone();
            let new_path = get_current_remote_path().lock().unwrap().clone();

            // Check preview data
            let new_preview = get_preview_data().lock().unwrap().clone();
            let current_preview_exists = preview_content.read().is_some();
            let new_preview_exists = new_preview.is_some();

            if current_preview_exists != new_preview_exists {
                preview_content.set(new_preview);
            } else if new_preview_exists {
                let should_update = if let Some(current) = preview_content.read().as_ref() {
                    if let Some(new_p) = new_preview.as_ref() {
                        current.filename != new_p.filename
                    } else {
                        false
                    }
                } else {
                    false
                };

                if should_update {
                    preview_content.set(new_preview);
                }
            }

            if global_update != *last_update_seen.read() {
                files.set(new_files);
                last_update_seen.set(global_update);
                loading.set(false);
            }

            if new_path != *current_path.read() {
                current_path.set(new_path);
            }
            async_std::task::sleep(std::time::Duration::from_millis(200)).await;
        }
    });

    let go_up = {
        let ip = device.ip.clone();
        let port = device.port;
        move |_| {
            let p = current_path.read().clone();
            if p != "/" {
                let parent = std::path::Path::new(&p)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or("/".to_string());
                let parent = if parent.is_empty() {
                    "/".to_string()
                } else {
                    parent
                };

                loading.set(true);
                action_tx.send(AppAction::ListRemoteFiles {
                    ip: ip.clone(),
                    port: port,
                    path: parent,
                });
            }
        }
    };

    rsx! {
        div {
            class: "file-browser",
            onclick: move |_| context_menu.set(None), // Close context menu on click elsewhere

            div {
                class: "browser-header",
                button { class: "secondary-button", onclick: move |_| on_close.call(()), "Back" }
                h3 { "Files on {device.name}" }
                span { class: "path-display", "{current_path}" }
            }

            if *loading.read() {
                 div { class: "loading", "Loading..." }
            } else if let Some(entries) = files.read().as_ref() {
                div {
                    class: "file-list",
                    if current_path.read().as_str() != "/" {
                        div {
                            class: "file-entry directory",
                            onclick: go_up,
                            span { class: "icon", "📁" }
                            span { class: "name", ".." }
                        }
                    }
                    for entry in entries {
                        div {
                            class: "file-entry {entry.entry_type:?}",
                            onclick: {
                                let entry = entry.clone();
                                let ip = device.ip.clone();
                                let port = device.port;
                                move |evt: Event<MouseData>| {
                                    // Handle left click normally
                                    if let FsEntryType::Directory = entry.entry_type {
                                        loading.set(true);
                                        action_tx.send(AppAction::ListRemoteFiles {
                                            ip: ip.clone(),
                                            port: port,
                                            path: entry.path.clone(),
                                        });
                                    } else {
                                        action_tx.send(AppAction::DownloadFile {
                                            ip: ip.clone(),
                                            port: port,
                                            remote_path: entry.path.clone(),
                                            filename: entry.name.clone(),
                                        });
                                    }
                                }
                            },
                            oncontextmenu: {
                                let entry = entry.clone();
                                move |evt: Event<MouseData>| {
                                    evt.prevent_default();
                                    if let FsEntryType::File = entry.entry_type {
                                        let coords = evt.client_coordinates();
                                        context_menu.set(Some((entry.path.clone(), entry.name.clone(), coords.x as i32, coords.y as i32)));
                                    }
                                }
                            },
                            span {
                                class: "icon",
                                match entry.entry_type {
                                    FsEntryType::Directory => "📁",
                                    _ => "📄",
                                }
                            }
                            span { class: "name", "{entry.name}" }
                            span { class: "size", "{format_size(entry.size)}" }
                        }
                    }
                }
            } else {
                 div { class: "empty", "No files or connection error" }
            }

            if let Some((path, name, x, y)) = context_menu.read().as_ref() {
                div {
                    class: "context-menu",
                    style: "top: {y}px; left: {x}px; position: absolute; background: var(--bg-card); border: 1px solid var(--border); border-radius: 8px; z-index: 1000; box-shadow: 0 4px 12px rgba(0,0,0,0.2);",
                    div {
                        class: "menu-item",
                        onclick: {
                            let ip = device.ip.clone();
                            let port = device.port;
                            let path = path.clone();
                            let name = name.clone();
                            move |evt: Event<MouseData>| {
                                evt.stop_propagation();
                                context_menu.set(None);
                                action_tx.send(AppAction::PreviewFile {
                                    ip: ip.clone(),
                                    port,
                                    remote_path: path.clone(),
                                    filename: name.clone(),
                                });
                            }
                        },
                        "👁️ Preview"
                    }
                    div {
                        class: "menu-item",
                        onclick: {
                            let ip = device.ip.clone();
                            let port = device.port;
                            let path = path.clone();
                            let name = name.clone();
                            move |evt: Event<MouseData>| {
                                evt.stop_propagation();
                                context_menu.set(None);
                                action_tx.send(AppAction::DownloadFile {
                                    ip: ip.clone(),
                                    port,
                                    remote_path: path.clone(),
                                    filename: name.clone(),
                                });
                            }
                        },
                        "📥 Download"
                    }
                }
            }

            if let Some(data) = preview_content.read().as_ref() {
                div {
                    class: "modal-overlay",
                    style: "position: fixed; top: 0; left: 0; right: 0; bottom: 0; background: rgba(0,0,0,0.8); z-index: 2000; display: flex; align-items: center; justify-content: center;",
                    onclick: move |_| action_tx.send(AppAction::ClosePreview),
                    div {
                        class: "preview-modal",
                        style: "background: var(--bg-card); padding: 16px; border-radius: 8px; max-width: 90vw; max-height: 90vh; display: flex; flex-direction: column;",
                        onclick: |evt| evt.stop_propagation(),
                        div {
                            class: "preview-header",
                            style: "display: flex; justify-content: space-between; margin-bottom: 16px;",
                            h3 { "{data.filename}" }
                            button {
                                style: "background: none; border: none; font-size: 1.5rem; cursor: pointer; color: var(--text-primary);",
                                onclick: move |_| action_tx.send(AppAction::ClosePreview),
                                "✕"
                            }
                        }
                        div {
                            class: "preview-content",
                            style: "overflow: auto; flex: 1; display: flex; align-items: center; justify-content: center;",
                            if data.mime_type.starts_with("image/") {
                                img {
                                    src: "data:{data.mime_type};base64,{base64::engine::general_purpose::STANDARD.encode(&data.data)}",
                                    style: "max-width: 100%; max-height: 80vh; object-fit: contain;"
                                }
                            } else if data.mime_type.starts_with("text/") {
                                pre {
                                    style: "white-space: pre-wrap; font-family: monospace; text-align: left;",
                                    "{String::from_utf8_lossy(&data.data)}"
                                }
                            } else {
                                div {
                                    style: "text-align: center; padding: 32px;",
                                    p { "Preview not supported for {data.mime_type}" }
                                    p { "Size: {format_size(data.data.len() as u64)}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let i = (bytes as f64).log(1024.0).floor() as usize;
    let i = i.min(UNITS.len() - 1);
    let s = bytes as f64 / 1024.0f64.powi(i as i32);
    format!("{:.1} {}", s, UNITS[i])
}
