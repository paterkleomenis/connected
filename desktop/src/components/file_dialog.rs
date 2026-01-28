use crate::components::icon::{Icon, IconType};
use crate::state::DeviceInfo;
use dioxus::html::HasFileData;
use dioxus::prelude::*;

#[component]
pub fn FileDialog(
    device: Option<DeviceInfo>,
    is_folder: bool,
    on_close: EventHandler<()>,
    on_send: EventHandler<String>,
) -> Element {
    let mut file_path = use_signal(String::new);
    let mut drag_over = use_signal(|| false);
    let mut is_browsing = use_signal(|| false);

    let browse_file = move |_| {
        if *is_browsing.read() {
            return;
        }
        is_browsing.set(true);

        let mut file_path = file_path;
        let mut drag_over = drag_over;
        let mut is_browsing = is_browsing;

        // On Windows, the sync dialog can fail to open if COM is already initialized in a
        // different apartment mode on the UI thread (common with WebView-based apps).
        // The async dialog runs on a dedicated thread and avoids freezing/crashing the UI.
        spawn(async move {
            let dialog = rfd::AsyncFileDialog::new();

            let selected = if is_folder {
                dialog.pick_folder().await
            } else {
                dialog.pick_file().await
            };

            if let Some(handle) = selected {
                file_path.set(handle.path().display().to_string());
                drag_over.set(false);
            }

            is_browsing.set(false);
        });
    };

    let device_name = device
        .as_ref()
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "Unknown Device".to_string());

    let has_file = !file_path.read().is_empty();

    let title_text = if is_folder {
        " Send Folder"
    } else {
        " Send File"
    };
    let drop_text = if is_folder {
        "Click to browse a folder"
    } else {
        "Click to browse or drag a file here"
    };
    let support_text = if is_folder {
        "Folder will be sent as a zip archive"
    } else {
        "Supports all file types"
    };
    let button_text = if is_folder {
        " Send Folder"
    } else {
        " Send File"
    };
    let select_text = if is_folder {
        "Select a Folder"
    } else {
        "Select a File"
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

                            let dropped = evt
                                .data()
                                .as_ref()
                                .files()
                                .into_iter()
                                .next()
                                .map(|f| f.path());

                            if let Some(path) = dropped {
                                let ok = if is_folder { path.is_dir() } else { path.is_file() };
                                if ok {
                                    file_path.set(path.display().to_string());
                                }
                            }
                        },

                        if !has_file {
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
                            p { class: "file-path", "{file_path}" }
                            button {
                                class: "clear-button",
                                onclick: move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    file_path.set(String::new());
                                },
                                Icon { icon: IconType::Close, size: 12, color: "currentColor".to_string() }
                                span { " Remove" }
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
                            disabled: !has_file,
                            onclick: {
                                let path = file_path.read().clone();
                                move |_| {
                                    if !path.is_empty() {
                                        on_send.call(path.clone());
                                    }
                                }
                            },
                            if has_file {
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
