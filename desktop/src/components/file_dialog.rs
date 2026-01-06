use crate::state::DeviceInfo;
use dioxus::prelude::*;

#[component]
pub fn FileDialog(
    device: Option<DeviceInfo>,
    on_close: EventHandler<()>,
    on_send: EventHandler<String>,
) -> Element {
    let mut file_path = use_signal(String::new);
    #[allow(unused_variables)]
    let drag_over = use_signal(|| false);

    let browse_file = move |_| {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            file_path.set(path.display().to_string());
        }
    };

    let device_name = device.as_ref().map(|d| d.name.clone()).unwrap_or_default();

    rsx! {
        div {
            class: "dialog-overlay",
            onclick: move |_| on_close.call(()),

            div {
                class: "dialog",
                onclick: move |evt| evt.stop_propagation(),

                div {
                    class: "dialog-header",
                    h2 { "Send File" }
                    button {
                        class: "dialog-close",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }

                div {
                    class: "dialog-content",

                    p { class: "dialog-subtitle", "Send to: {device_name}" }

                    div {
                        class: if *drag_over.read() { "drop-zone drag-over" } else { "drop-zone" },
                        onclick: browse_file,

                        if file_path.read().is_empty() {
                            div { class: "drop-icon", "📂" }
                            p { "Click to browse or drag a file here" }
                        } else {
                            div { class: "drop-icon", "📄" }
                            p { class: "file-path", "{file_path}" }
                            button {
                                class: "clear-button",
                                onclick: move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    file_path.set(String::new());
                                },
                                "✕ Clear"
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
                            disabled: file_path.read().is_empty(),
                            onclick: {
                                let path = file_path.read().clone();
                                move |_| {
                                    if !path.is_empty() {
                                        on_send.call(path.clone());
                                    }
                                }
                            },
                            "Send File"
                        }
                    }
                }
            }
        }
    }
}
