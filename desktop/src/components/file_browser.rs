use crate::controller::AppAction;
use crate::state::{
    get_current_remote_files, get_current_remote_path, get_remote_files_update, DeviceInfo,
};
use connected_core::filesystem::{FsEntry, FsEntryType};
use dioxus::prelude::*;

#[component]
pub fn FileBrowser(device: DeviceInfo, on_close: EventHandler<()>) -> Element {
    let action_tx = use_coroutine_handle::<AppAction>();
    let mut current_path = use_signal(|| get_current_remote_path().lock().unwrap().clone());
    let mut files = use_signal(|| Option::<Vec<FsEntry>>::None);
    let mut loading = use_signal(|| false);
    let mut last_update_seen = use_signal(|| *get_remote_files_update().lock().unwrap());

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
                                move |_| {
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
