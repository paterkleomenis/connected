use crate::state::DeviceInfo;
use crate::utils::get_device_icon;
use dioxus::prelude::*;

#[component]
pub fn DeviceCard(
    device: DeviceInfo,
    is_selected: bool,
    on_select: EventHandler<DeviceInfo>,
    on_pair: EventHandler<DeviceInfo>,
) -> Element {
    let icon = get_device_icon(&device.device_type);

    rsx! {
        div {
            class: if is_selected { "device-card selected" } else { "device-card" },
            onclick: {
                let device = device.clone();
                move |_| {
                    if device.is_trusted {
                        on_select.call(device.clone())
                    }
                }
            },

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
                if device.is_trusted {
                    p { class: "device-status trusted", "✓ Trusted" }
                } else {
                    p { class: "device-status untrusted", "Not Trusted" }
                }
            }

            // Only show Pair button for untrusted devices
            if !device.is_trusted {
                div {
                    class: "device-actions",
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
                }
            }
        }
    }
}
