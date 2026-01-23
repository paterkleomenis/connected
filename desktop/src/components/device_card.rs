use crate::components::icon::{Icon, IconType, get_device_icon_type};
use crate::state::DeviceInfo;
use dioxus::prelude::*;

#[component]
pub fn DeviceCard(
    device: DeviceInfo,
    is_selected: bool,
    on_select: EventHandler<DeviceInfo>,
    on_pair: EventHandler<DeviceInfo>,
    on_send_file: EventHandler<DeviceInfo>,
) -> Element {
    let device_icon = get_device_icon_type(&device.device_type);
    let mut is_hovered = use_signal(|| false);

    let card_class = if is_selected {
        "device-card selected"
    } else {
        "device-card"
    };

    rsx! {
        div {
            class: "{card_class}",
            onmouseenter: move |_| is_hovered.set(true),
            onmouseleave: move |_| is_hovered.set(false),
            onclick: {
                let device = device.clone();
                move |_| {
                    if device.is_trusted {
                        on_select.call(device.clone())
                    }
                }
            },

            div {
                class: "device-card-icon",
                Icon { icon: device_icon.clone(), size: 28, color: "var(--text-primary)".to_string() }
                if !device.is_trusted {
                    span {
                        class: "untrusted-badge",
                        Icon { icon: IconType::Warning, size: 12, color: "var(--bg-card)".to_string() }
                    }
                }
            }

            div {
                class: "device-card-info",
                h3 { class: "device-name", "{device.name}" }
                p { class: "device-address",
                    if device.ip == "0.0.0.0" {
                        "Offline (BLE)"
                    } else {
                        "{device.ip}:{device.port}"
                    }
                }
                if device.device_type.to_lowercase() != "unknown" {
                    p { class: "device-type", "{device.device_type}" }
                }
                if device.is_trusted {
                    p {
                        class: "device-status trusted",
                        Icon { icon: IconType::Check, size: 12, color: "var(--success)".to_string() }
                        span { " Trusted" }
                    }
                } else {
                    p {
                        class: "device-status untrusted",
                        Icon { icon: IconType::Untrusted, size: 12, color: "var(--text-tertiary)".to_string() }
                        span { " Not paired" }
                    }
                }
            }

            if !device.is_trusted {
                div {
                    class: "device-actions",
                    if device.is_pending {
                        button {
                            class: "action-button pair disabled",
                            disabled: true,
                            Icon { icon: IconType::Sync, size: 14, color: "var(--text-secondary)".to_string() }
                            span { " Waiting..." }
                        }
                    } else {
                        button {
                            class: "action-button pair",
                            title: "Pair with this device",
                            onclick: {
                                let device = device.clone();
                                move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    on_pair.call(device.clone());
                                }
                            },
                            Icon { icon: IconType::Pair, size: 14, color: "currentColor".to_string() }
                            span { " Pair" }
                        }
                        button {
                            class: "action-button",
                            title: "Send a file without pairing",
                            onclick: {
                                let device = device.clone();
                                move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    on_send_file.call(device.clone());
                                }
                            },
                            Icon { icon: IconType::Send, size: 14, color: "currentColor".to_string() }
                            span { " Send" }
                        }
                    }
                }
            }

            if device.is_trusted && *is_hovered.read() {
                div {
                    class: "device-actions",
                    button {
                        class: "action-button",
                        title: "Open device",
                        onclick: {
                            let device = device.clone();
                            move |evt: Event<MouseData>| {
                                evt.stop_propagation();
                                on_select.call(device.clone());
                            }
                        },
                        Icon { icon: IconType::ArrowRight, size: 16, color: "currentColor".to_string() }
                    }
                }
            }
        }
    }
}
