use crate::components::icon::{Icon, IconType};
use crate::state::DeviceInfo;
use dioxus::html::HasFileData;
use dioxus::prelude::*;

#[component]
pub fn FileDialog(
    device: Option<DeviceInfo>,
    is_folder: bool,
    on_close: EventHandler<()>,
    on_send: EventHandler<Vec<String>>,
) -> Element {
    let mut file_paths = use_signal(Vec::<String>::new);
    let mut drag_over = use_signal(|| false);
    let mut is_browsing = use_signal(|| false);

    let browse_file = move |_| {
        if *is_browsing.read() {
            return;
        }
        is_browsing.set(true);

        let mut file_paths = file_paths;
        let mut drag_over = drag_over;
        let mut is_browsing = is_browsing;

        spawn(async move {
            let dialog = rfd::AsyncFileDialog::new();

            if is_folder {
                if let Some(handle) = dialog.pick_folder().await {
                    file_paths.set(vec![handle.path().display().to_string()]);
                    drag_over.set(false);
                }
            } else {
                if let Some(handles) = dialog.pick_files().await {
                    let paths = handles.into_iter().map(|h| h.path().display().to_string()).collect();
                    file_paths.set(paths);
                    drag_over.set(false);
                }
            };

            is_browsing.set(false);
        });
    };

    let device_name = device
        .as_ref()
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "Unknown Device".to_string());

    let has_files = !file_paths.read().is_empty();

    let title_text = if is_folder {
        " Send Folder"
    } else {
        " Send Files"
    };
    let drop_text = if is_folder {
        "Click to browse a folder"
    } else {
        "Click to browse or drag files here"
    };
    let support_text = if is_folder {
        "Folder will be sent as a zip archive"
    } else {
        "Supports multiple files and types"
    };
    let button_text = if is_folder {
        " Send Folder".to_string()
    } else {
        if file_paths.read().len() > 1 {
            format!(" Send {} Files", file_paths.read().len())
        } else {
            " Send File".to_string()
        }
    };
    let select_text = if is_folder {
        "Select a Folder"
    } else {
        "Select Files"
    };

    rsx! {
        div {
            class: "dialog-overlay",
            onclick: move |_| on_close.call(()),

            div {
                class: "dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "dialog-header",
                    h2 {
                        Icon { icon: IconType::Send, size: 18, color: "var(--accent)".to_string() }
                        span { "{title_text}" }
                    }
                    button {
                        class: "dialog-close",
                        onclick: move |_| on_close.call(()),
                        Icon { icon: IconType::Close, size: 16, color: "currentColor".to_string() }
                    }
                }

                div {
                    class: "dialog-content",

                    p {
                        class: "dialog-subtitle",
                        "Send to: "
                        strong { "{device_name}" }
                    }

                    div {
                        class: if *drag_over.read() { "drop-zone drag-over" } else { "drop-zone" },
                        onclick: browse_file,
                        ondragover: move |evt| {
                            evt.prevent_default();
                            drag_over.set(true);
                        },
                        ondragleave: move |_| {
                            drag_over.set(false);
                        },
                        ondrop: move |evt| {
                            evt.prevent_default();
                            drag_over.set(false);

                            let dropped: Vec<_> = evt
                                .data()
                                .as_ref()
                                .files()
                                .into_iter()
                                .map(|f| f.path())
                                .collect();

                            if !dropped.is_empty() {
                                let mut valid_paths = Vec::new();
                                for path in dropped {
                                    let ok = if is_folder { path.is_dir() } else { path.is_file() };
                                    if ok {
                                        valid_paths.push(path.display().to_string());
                                    }
                                }
                                if !valid_paths.is_empty() {
                                    file_paths.set(valid_paths);
                                }
                            }
                        },

                        if !has_files {
                            div {
                                class: "drop-icon",
                                Icon { icon: IconType::Upload, size: 48, color: "var(--text-tertiary)".to_string() }
                            }
                            p { "{drop_text}" }
                            p { class: "muted", style: "font-size: 12px; margin-top: 8px;", "{support_text}" }
                        } else {
                            div {
                                class: "drop-icon",
                                Icon { icon: IconType::Check, size: 48, color: "var(--success)".to_string() }
                            }
                            div {
                                class: "file-paths-list",
                                if file_paths.read().len() > 3 {
                                    p { class: "file-path", "{file_paths.read()[0]}" }
                                    p { class: "file-path", "{file_paths.read()[1]}" }
                                    p { class: "file-path muted", "... and {file_paths.read().len() - 2} more" }
                                } else {
                                    for path in file_paths.read().iter() {
                                        p { class: "file-path", "{path}" }
                                    }
                                }
                            }
                            button {
                                class: "clear-button",
                                onclick: move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    file_paths.set(Vec::new());
                                },
                                Icon { icon: IconType::Close, size: 12, color: "currentColor".to_string() }
                                span { " Remove All" }
                            }
                        }
                    }

                    div {
                        class: "dialog-actions",
                        button {
                            class: "secondary-button",
                            onclick: move |_| on_close.call(()),
                            "Cancel"
                        }
                        button {
                            class: "primary-button",
                            disabled: !has_files,
                            onclick: {
                                let paths = file_paths.read().clone();
                                move |_| {
                                    if !paths.is_empty() {
                                        on_send.call(paths.clone());
                                    }
                                }
                            },
                            if has_files {
                                Icon { icon: IconType::Send, size: 14, color: "currentColor".to_string() }
                                span { "{button_text}" }
                            } else {
                                span { "{select_text}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
