use dioxus::prelude::*;

#[component]
pub fn ControlHelp() -> Element {
    rsx! {
        style { {include_str!("../assets/control-help.css")} }
        div {
            id: "editor-control-help", class: "control-help", role: "tooltip", hidden: true,
            onmounted: move |_| { let _ = document::eval(include_str!("control_help.js")); },
        }
    }
}
