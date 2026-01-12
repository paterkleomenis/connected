use crate::components::icon::{get_file_icon_type, Icon, IconType};
use crate::controller::AppAction;
use crate::state::{
    get_current_remote_files, get_current_remote_path, get_preview_data, get_remote_files_update,
    DeviceInfo, PreviewData,
};
use crate::utils::format_file_size;
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

    use_effect(use_reactive(&device, move |device| {
        loading.set(true);
        action_tx.send(AppAction::ListRemoteFiles {
            ip: device.ip.clone(),
            port: device.port,
            path: "/".to_string(),
        });
    }));

    use_future(move || async move {
        loop {
            let global_update = *get_remote_files_update().lock().unwrap();
            let new_files = get_current_remote_files().lock().unwrap().clone();
            let new_path = get_current_remote_path().lock().unwrap().clone();

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
                    port,
                    path: parent,
                });
            }
        }
    };

    rsx! {
        div {
            class: "file-browser",
            onclick: move |_| context_menu.set(None),

            div {
                class: "browser-header",
                button {
                    class: "secondary-button",
                    onclick: move |_| on_close.call(()),
                    Icon { icon: IconType::Back, size: 14, color: "currentColor".to_string() }
                    span { " Back" }
                }
                h3 { "Files on {device.name}" }
                span { class: "path-display", "{current_path}" }
            }

            if *loading.read() {
                div {
                    class: "loading",
                    div { class: "searching-indicator",
                        span { class: "dot" }
                        span { class: "dot" }
                        span { class: "dot" }
                    }
                    span { "Loading files..." }
                }
            } else if let Some(entries) = files.read().as_ref() {
                div {
                    class: "file-list",
                    if current_path.read().as_str() != "/" {
                        div {
                            class: "file-entry directory",
                            onclick: go_up,
                            span {
                                class: "icon",
                                Icon { icon: IconType::Folder, size: 18, color: "var(--accent)".to_string() }
                            }
                            span { class: "name", ".." }
                            span { class: "size", "" }
                        }
                    }
                    for entry in entries {
                        {
                            let entry_class = match entry.entry_type {
                                FsEntryType::Directory => "file-entry directory",
                                _ => "file-entry file",
                            };
                            let icon_type = match entry.entry_type {
                                FsEntryType::Directory => IconType::Folder,
                                _ => get_file_icon_type(&entry.name),
                            };
                            let icon_color = match entry.entry_type {
                                FsEntryType::Directory => "var(--accent)",
                                _ => "var(--text-secondary)",
                            };
                            rsx! {
                                div {
                                    class: "{entry_class}",
                                    onclick: {
                                        let entry = entry.clone();
                                        let ip = device.ip.clone();
                                        let port = device.port;
                                        move |_evt: Event<MouseData>| {
                                            if let FsEntryType::Directory = entry.entry_type {
                                                loading.set(true);
                                                action_tx.send(AppAction::ListRemoteFiles {
                                                    ip: ip.clone(),
                                                    port,
                                                    path: entry.path.clone(),
                                                });
                                            } else {
                                                action_tx.send(AppAction::DownloadFile {
                                                    ip: ip.clone(),
                                                    port,
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
                                                context_menu.set(Some((
                                                    entry.path.clone(),
                                                    entry.name.clone(),
                                                    coords.x as i32,
                                                    coords.y as i32
                                                )));
                                            }
                                        }
                                    },
                                    span {
                                        class: "icon",
                                        Icon { icon: icon_type, size: 18, color: icon_color.to_string() }
                                    }
                                    span { class: "name", "{entry.name}" }
                                    span { class: "size", "{format_file_size(entry.size)}" }
                                }
                            }
                        }
                    }
                }
            } else {
                div {
                    class: "empty",
                    Icon { icon: IconType::Folder, size: 48, color: "var(--text-tertiary)".to_string() }
                    p { "No files found or connection error" }
                }
            }

            if let Some((path, name, x, y)) = context_menu.read().as_ref() {
                div {
                    class: "context-menu",
                    style: "top: {y}px; left: {x}px;",
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
                        Icon { icon: IconType::Search, size: 14, color: "currentColor".to_string() }
                        span { " Preview" }
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
                        Icon { icon: IconType::Download, size: 14, color: "currentColor".to_string() }
                        span { " Download" }
                    }
                }
            }

            if let Some(data) = preview_content.read().as_ref() {
                div {
                    class: "modal-overlay",
                    onclick: move |_| action_tx.send(AppAction::ClosePreview),
                    div {
                        class: "modal-content",
                        style: "max-width: 90vw; max-height: 90vh; overflow: hidden;",
                        onclick: |evt| evt.stop_propagation(),

                        div {
                            class: "dialog-header",
                            h2 { "{data.filename}" }
                            button {
                                class: "dialog-close",
                                onclick: move |_| action_tx.send(AppAction::ClosePreview),
                                Icon { icon: IconType::Close, size: 16, color: "currentColor".to_string() }
                            }
                        }

                        div {
                            class: "dialog-content",
                            style: "overflow: auto; max-height: 70vh; display: flex; align-items: center; justify-content: center;",
                            if data.mime_type.starts_with("image/") {
                                img {
                                    src: "data:{data.mime_type};base64,{base64::engine::general_purpose::STANDARD.encode(&data.data)}",
                                    style: "max-width: 100%; max-height: 65vh; object-fit: contain; border-radius: 8px;"
                                }
                            } else if data.mime_type.starts_with("text/") {
                                pre {
                                    style: "white-space: pre-wrap; font-family: var(--font-mono); text-align: left; padding: 16px; background: var(--bg-tertiary); border-radius: 8px; width: 100%; overflow-x: auto;",
                                    "{String::from_utf8_lossy(&data.data)}"
                                }
                            } else {
                                                                    div {
                                                                    class: "empty-state",
                                                                    Icon { icon: IconType::File, size: 48, color: "var(--text-tertiary)".to_string() }
                                                                    p { "Preview not available for {data.mime_type}" }
                                                                    p { class: "muted", "Size: {format_file_size(data.data.len() as u64)}" }
                                                                }
                                                            }
                                                        }                    }
                }
            }
        }
    }
}
