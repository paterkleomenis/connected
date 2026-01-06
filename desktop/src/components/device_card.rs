use crate::state::DeviceInfo;
use crate::utils::get_device_icon;
use dioxus::prelude::*;
use std::thread;

#[component]
pub fn DeviceCard(
    device: DeviceInfo,
    is_selected: bool,
    on_select: EventHandler<DeviceInfo>,
    on_send_file: EventHandler<DeviceInfo>,
    on_send_clipboard: EventHandler<DeviceInfo>,
) -> Element {
    let mut pinging = use_signal(|| false);
    let mut ping_result = use_signal(|| None::<String>);
    let mut show_actions = use_signal(|| false);

    let device_for_ping = device.clone();
    let handle_ping = move |_| {
        if *pinging.read() {
            return;
        }

        let ip = device_for_ping.ip.clone();
        let port = device_for_ping.port;

        pinging.set(true);
        ping_result.set(None);

        let result = thread::spawn(move || connected_core::facade::send_ping(ip, port))
            .join()
            .ok();

        if let Some(res) = result {
            if res.success {
                ping_result.set(Some(format!("{}ms", res.rtt_ms)));
            } else {
                ping_result.set(Some("Failed".into()));
            }
        } else {
            ping_result.set(Some("Error".into()));
        }
        pinging.set(false);
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
            }

            // Device info
            div {
                class: "device-card-info",
                h3 { class: "device-name", "{device.name}" }
                p { class: "device-address", "{device.ip}:{device.port}" }
                p { class: "device-type", "{device.device_type}" }
            }

            // Ping result
            if let Some(ref result) = *ping_result.read() {
                span {
                    class: if result.contains("ms") { "ping-badge success" } else { "ping-badge error" },
                    "{result}"
                }
            }

            // Actions overlay
            if *show_actions.read() || is_selected {
                div {
                    class: "device-actions",
                    button {
                        class: "action-button",
                        title: "Ping",
                        onclick: handle_ping,
                        disabled: *pinging.read(),
                        if *pinging.read() { "..." } else { "📶" }
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
                }
            }
        }
    }
}
