use crate::components::icon::{Icon, IconType, get_file_icon_type};
use crate::controller::AppAction;
use crate::state::{
    DeviceInfo, LockOrRecover, PreviewData, get_current_remote_files, get_current_remote_path,
    get_preview_data, get_remote_files_update, get_thumbnails, get_thumbnails_update, send_action,
};
use crate::utils::format_file_size;
use base64::Engine as _;
use connected_core::filesystem::{FsEntry, FsEntryType};
use dioxus::prelude::*;
use std::collections::HashMap;

#[component]
pub fn FileBrowser(device: DeviceInfo, on_close: EventHandler<()>) -> Element {
    let mut current_path = use_signal(|| get_current_remote_path().lock_or_recover().clone());
    let mut files = use_signal(|| Option::<Vec<FsEntry>>::None);
    let mut loading = use_signal(|| false);
    let mut last_update_seen = use_signal(|| *get_remote_files_update().lock_or_recover());
    let mut context_menu = use_signal(|| Option::<(String, String, i32, i32)>::None);
    let mut preview_content = use_signal(|| Option::<PreviewData>::None);

    // Thumbnail state
    let mut current_thumbnails = use_signal(HashMap::<String, String>::new); // path -> base64
    let mut last_thumbnails_update = use_signal(|| *get_thumbnails_update().lock_or_recover());
    let mut requested_thumbnails = use_signal(HashMap::<String, std::time::Instant>::new);

    use_effect(use_reactive(&device, move |device| {
        loading.set(true);
        send_action(AppAction::ListRemoteFiles {
            ip: device.ip.clone(),
            port: device.port,
            path: "/".to_string(),
        });
    }));

    // Sync state from global stores
    use_future(move || async move {
        loop {
            // Get data from global mutexes
            let (global_update, thumbnails_ts, new_files, new_path, new_preview) = {
                let files_update = *get_remote_files_update().lock_or_recover();
                let thumbs_update = *get_thumbnails_update().lock_or_recover();
                let files_list = get_current_remote_files().lock_or_recover().clone();
                let path = get_current_remote_path().lock_or_recover().clone();
                let preview = get_preview_data().lock_or_recover().clone();
                (files_update, thumbs_update, files_list, path, preview)
            };

            // Update preview if changed
            let current_preview = preview_content.read();
            let should_update_preview = match (current_preview.as_ref(), new_preview.as_ref()) {
                (None, Some(_)) | (Some(_), None) => true,
                (Some(c), Some(n)) => c.filename != n.filename,
                (None, None) => false,
            };
            drop(current_preview);

            if should_update_preview {
                preview_content.set(new_preview);
            }

            // Update files if changed
            if global_update != *last_update_seen.read() {
                files.set(new_files);
                last_update_seen.set(global_update);
                loading.set(false);
            }

            // Update path if changed
            if new_path != *current_path.read() {
                current_path.set(new_path);
            }

            // Sync thumbnails if updated
            if thumbnails_ts != *last_thumbnails_update.read() {
                let thumbs_lock = get_thumbnails().lock_or_recover();
                let mut new_thumbs = HashMap::new();
                for (k, v) in thumbs_lock.iter() {
                    new_thumbs.insert(
                        k.clone(),
                        base64::engine::general_purpose::STANDARD.encode(v),
                    );
                }
                current_thumbnails.set(new_thumbs);
                last_thumbnails_update.set(thumbnails_ts);
            }

            // Allow thumbnail retries by expiring old requests
            {
                let mut requested = requested_thumbnails.write();
                requested.retain(|_, ts| ts.elapsed() < std::time::Duration::from_secs(5));
            }

            // Increased from 200ms to 500ms to reduce CPU usage
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    });

    // Request thumbnails when files change
    let eff_device = device.clone();
    use_effect(move || {
        let entries_opt = files.read();
        if let Some(entries) = entries_opt.as_ref() {
            let thumbs = current_thumbnails.read();
            let mut to_request = Vec::new();

            {
                let requested = requested_thumbnails.read();
                for entry in entries {
                    let is_image = ["jpg", "jpeg", "png", "gif", "webp", "bmp"].contains(
                        &entry
                            .name
                            .split('.')
                            .next_back()
                            .unwrap_or("")
                            .to_lowercase()
                            .as_str(),
                    );
                    if is_image
                        && matches!(entry.entry_type, FsEntryType::File)
                        && !thumbs.contains_key(&entry.path)
                        && !requested.contains_key(&entry.path)
                    {
                        to_request.push(entry.clone());
                    }
                }
            }

            if !to_request.is_empty() {
                let mut requested = requested_thumbnails.write();
                for entry in to_request {
                    requested.insert(entry.path.clone(), std::time::Instant::now());
                    send_action(AppAction::GetThumbnail {
                        ip: eff_device.ip.clone(),
                        port: eff_device.port,
                        path: entry.path,
                    });
                }
            }
        }
    });

    // Read signals once at the top of render to minimize borrow time
    let current_path_val = current_path.read();
    let files_val = files.read();
    let loading_val = *loading.read();
    let thumbnails_val = current_thumbnails.read();
    let preview_val = preview_content.read();
    let context_menu_val = context_menu.read();

    rsx! {
        div {
            class: "file-browser",
            onclick: move |_| context_menu.set(None),

            div {
                class: "browser-header",
                button {
                    class: "secondary-button",
                    onclick: {
                        let ip = device.ip.clone();
                        let port = device.port;
                        let p = current_path_val.clone();
                        move |_| {
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
                                send_action(AppAction::ListRemoteFiles {
                                    ip: ip.clone(),
                                    port,
                                    path: parent,
                                });
                            } else {
                                on_close.call(());
                            }
                        }
                    },
                    Icon { icon: IconType::Back, size: 14, color: "currentColor".to_string() }
                    span { " Back" }
                }
                h3 { "Files on {device.name}" }
                span { class: "path-display", "{current_path_val}" }
            }

            if loading_val {
                div {
                    class: "loading",
                    div { class: "searching-indicator",
                        span { class: "dot" }
                        span { class: "dot" }
                        span { class: "dot" }
                    }
                    span { "Loading files..." }
                }
            } else if let Some(entries) = files_val.as_ref() {
                div {
                    class: "file-list",
                    if current_path_val.as_str() != "/" {
                        div {
                            class: "file-entry directory",
                            onclick: {
                                let ip = device.ip.clone();
                                let port = device.port;
                                let p = current_path_val.clone();
                                move |_| {
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
                                        send_action(AppAction::ListRemoteFiles {
                                            ip: ip.clone(),
                                            port,
                                            path: parent,
                                        });
                                    }
                                }
                            },
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
                                                send_action(AppAction::ListRemoteFiles {
                                                    ip: ip.clone(),
                                                    port,
                                                    path: entry.path.clone(),
                                                });
                                            } else {
                                                send_action(AppAction::PreviewFile {
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
                                        if let Some(thumbnail_data) = thumbnails_val.get(&entry.path) {
                                            img {
                                                src: "data:image/jpeg;base64,{thumbnail_data}",
                                                style: "width: 24px; height: 24px; object-fit: cover; border-radius: 4px; display: block;"
                                            }
                                        } else {
                                            Icon { icon: icon_type, size: 18, color: icon_color.to_string() }
                                        }
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

            if let Some((path, name, x, y)) = context_menu_val.as_ref() {
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
                                send_action(AppAction::DownloadFile {
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

            if let Some(data) = preview_val.as_ref() {
                div {
                    class: "modal-overlay",
                    onclick: move |_| send_action(AppAction::ClosePreview),
                    div {
                        class: "modal-content",
                        style: "max-width: 90vw; max-height: 90vh; overflow: hidden;",
                        onclick: |evt| evt.stop_propagation(),

                        div {
                            class: "dialog-header",
                            h2 { "{data.filename}" }
                            button {
                                class: "dialog-close",
                                onclick: move |_| send_action(AppAction::ClosePreview),
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
                        }
                    }
                }
            }
        }
    }
}
