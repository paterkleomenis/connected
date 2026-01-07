use crate::state::DeviceInfo;
use crate::utils::get_device_icon;
use dioxus::prelude::*;

#[component]
pub fn DeviceCard(
    device: DeviceInfo,
    is_selected: bool,
    on_select: EventHandler<DeviceInfo>,
    on_send_file: EventHandler<DeviceInfo>,
    on_send_clipboard: EventHandler<DeviceInfo>,
    on_pair: EventHandler<DeviceInfo>,
    on_unpair: EventHandler<DeviceInfo>,
    on_forget: EventHandler<DeviceInfo>,
    on_block: EventHandler<DeviceInfo>,
) -> Element {
    let mut show_more_actions = use_signal(|| false);
    let mut show_actions = use_signal(|| false);

    // Ping disabled for Phase 1 Refactor
    let handle_ping = move |_| {
        // Todo: Implement Ping via AppAction
    };

    let icon = get_device_icon(&device.device_type);

    rsx! {
        div {
            class: if is_selected { "device-card selected" } else { "device-card" },
            onclick: {
                let device = device.clone();
                move |_| on_select.call(device.clone())
            },
            onmouseenter: move |_| show_actions.set(true),
            onmouseleave: move |_| show_actions.set(false),

            // Device icon
            div {
                class: "device-card-icon",
                "{icon}"
                if !device.is_trusted {
                    span { class: "untrusted-badge", "⚠️" }
                }
            }

            // Device info
            div {
                class: "device-card-info",
                h3 { class: "device-name", "{device.name}" }
                p { class: "device-address", "{device.ip}:{device.port}" }
                p { class: "device-type", "{device.device_type}" }
                if !device.is_trusted {
                    p { class: "device-status untrusted", "Not Trusted" }
                }
            }

            // Actions overlay
            if *show_actions.read() || is_selected {
                div {
                    class: "device-actions",
                    if !device.is_trusted {
                        if device.is_pending {
                            button {
                                class: "action-button pair disabled",
                                disabled: true,
                                "⏳ Waiting..."
                            }
                        } else {
                            button {
                                class: "action-button pair",
                                title: "Pair with Device",
                                onclick: {
                                    let device = device.clone();
                                    move |evt: Event<MouseData>| {
                                        evt.stop_propagation();
                                        on_pair.call(device.clone());
                                    }
                                },
                                "🔗 Pair"
                            }
                        }
                    } else {
                        button {
                            class: "action-button",
                            title: "Ping (Disabled)",
                            onclick: handle_ping,
                            "📶"
                        }
                        button {
                            class: "action-button",
                            title: "Send File",
                            onclick: {
                                let device = device.clone();
                                move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    on_send_file.call(device.clone());
                                }
                            },
                            "📁"
                        }
                        button {
                            class: "action-button",
                            title: "Send Clipboard",
                            onclick: {
                                let device = device.clone();
                                move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    on_send_clipboard.call(device.clone());
                                }
                            },
                            "📋"
                        }
                        // More actions dropdown
                        div {
                            class: "action-dropdown",
                            button {
                                class: "action-button",
                                title: "More actions",
                                onclick: move |evt: Event<MouseData>| {
                                    evt.stop_propagation();
                                    let current = *show_more_actions.read();
                                    show_more_actions.set(!current);
                                },
                                "⋮"
                            }
                            if *show_more_actions.read() {
                                div {
                                    class: "dropdown-menu",
                                    button {
                                        class: "dropdown-item",
                                        onclick: {
                                            let device = device.clone();
                                            move |evt: Event<MouseData>| {
                                                evt.stop_propagation();
                                                show_more_actions.set(false);
                                                on_unpair.call(device.clone());
                                            }
                                        },
                                        "💔 Unpair"
                                    }
                                    button {
                                        class: "dropdown-item warning",
                                        onclick: {
                                            let device = device.clone();
                                            move |evt: Event<MouseData>| {
                                                evt.stop_propagation();
                                                show_more_actions.set(false);
                                                on_forget.call(device.clone());
                                            }
                                        },
                                        "🔄 Forget"
                                    }
                                    button {
                                        class: "dropdown-item danger",
                                        onclick: {
                                            let device = device.clone();
                                            move |evt: Event<MouseData>| {
                                                evt.stop_propagation();
                                                show_more_actions.set(false);
                                                on_block.call(device.clone());
                                            }
                                        },
                                        "🚫 Block"
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
