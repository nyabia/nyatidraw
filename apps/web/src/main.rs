#[cfg(target_arch = "wasm32")]
mod adapter;
#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
mod gpu;
#[cfg(target_arch = "wasm32")]
mod preview;
#[cfg(target_arch = "wasm32")]
mod runtime;
#[cfg(target_arch = "wasm32")]
mod tool_preferences;
#[cfg(target_arch = "wasm32")]
mod ui;

#[cfg(target_arch = "wasm32")]
fn main() {
    dioxus::launch(ui::App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("NyatiDraw Web: use dx build --web --package nyatidraw-web");
}
