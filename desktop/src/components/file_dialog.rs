use crate::components::icon::{Icon, IconType};
use crate::state::DeviceInfo;
use dioxus::prelude::*;

#[component]
pub fn FileDialog(
    device: Option<DeviceInfo>,
    on_close: EventHandler<()>,
    on_send: EventHandler<String>,
) -> Element {
    let mut file_path = use_signal(String::new);
    let mut drag_over = use_signal(|| false);

    let browse_file = move |_| {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            file_path.set(path.display().to_string());
        }
    };

    let device_name = device
        .as_ref()
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "Unknown Device".to_string());

    let has_file = !file_path.read().is_empty();

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
                        span { " Send File" }
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

                        if !has_file {
                            div {
                                class: "drop-icon",
                                Icon { icon: IconType::Upload, size: 48, color: "var(--text-tertiary)".to_string() }
                            }
                            p { "Click to browse or drag a file here" }
                            p { class: "muted", style: "font-size: 12px; margin-top: 8px;", "Supports all file types" }
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
                                span { " Send File" }
                            } else {
                                span { "Select a File" }
                            }
                        }
                    }
                }
            }
        }
    }
}
