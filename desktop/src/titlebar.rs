use crate::components::{Icon, IconType};
use dioxus::prelude::*;

#[component]
pub fn Titlebar() -> Element {
    let desktop_window = dioxus::desktop::use_window();
    let mut is_maximized = use_signal(|| desktop_window.window.is_maximized());

    let dw_drag = desktop_window.clone();
    let dw_dblclick = desktop_window.clone();
    let dw_min = desktop_window.clone();
    let dw_max = desktop_window.clone();
    let dw_close = desktop_window.clone();

    rsx! {
        div {
            class: "custom-titlebar",

            div {
                class: "titlebar-drag-area",
                onmousedown: move |_| { dw_drag.drag(); },
                ondoubleclick: move |_| {
                    dw_dblclick.toggle_maximized();
                    let val = !*is_maximized.read();
                    is_maximized.set(val);
                },
                div {
                    class: "titlebar-logo",
                    Icon { icon: IconType::Logo, size: 16, color: "currentColor".to_string() }
                }
                span { class: "titlebar-title", "Connected" }
            }

            div {
                class: "titlebar-controls",

                button {
                    class: "titlebar-btn minimize",
                    onclick: move |_| { dw_min.window.set_minimized(true); },
                    title: "Minimize",
                    svg {
                        width: "10",
                        height: "10",
                        view_box: "0 0 10 10",
                        line { x1: "1", y1: "5", x2: "9", y2: "5", stroke: "currentColor", stroke_width: "1.2" }
                    }
                }

                button {
                    class: "titlebar-btn maximize",
                    onclick: move |_| {
                        dw_max.toggle_maximized();
                        let val = !*is_maximized.read();
                        is_maximized.set(val);
                    },
                    title: if *is_maximized.read() { "Restore" } else { "Maximize" },
                    if *is_maximized.read() {
                        svg {
                            width: "10",
                            height: "10",
                            view_box: "0 0 12 12",
                            rect { x: "3", y: "0.5", width: "6.5", height: "6.5", fill: "var(--bg-sidebar-solid)", stroke: "currentColor", stroke_width: "1" }
                            rect { x: "0.5", y: "2.5", width: "6.5", height: "6.5", fill: "var(--bg-primary)", stroke: "currentColor", stroke_width: "1" }
                        }
                    } else {
                        svg {
                            width: "10",
                            height: "10",
                            view_box: "0 0 10 10",
                            rect { x: "1", y: "1", width: "8", height: "8", fill: "none", stroke: "currentColor", stroke_width: "1" }
                        }
                    }
                }

                button {
                    class: "titlebar-btn close",
                    onclick: move |_| { dw_close.close(); },
                    title: "Close",
                    svg {
                        width: "10",
                        height: "10",
                        view_box: "0 0 10 10",
                        line { x1: "1.5", y1: "1.5", x2: "8.5", y2: "8.5", stroke: "currentColor", stroke_width: "1.2" }
                        line { x1: "8.5", y1: "1.5", x2: "1.5", y2: "8.5", stroke: "currentColor", stroke_width: "1.2" }
                    }
                }
            }
        }
    }
}
