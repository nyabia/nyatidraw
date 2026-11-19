#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
include!("desktop_runtime_modules.rs");
#[cfg(windows)]
mod gpui_host;
#[cfg(windows)]
const GPUI_SHELL: bool = true;
#[cfg(windows)]
mod palette_preferences;

#[cfg(windows)]
pub(crate) fn elapsed_since_launch() -> u128 {
    use std::{sync::OnceLock, time::Instant};
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis()
}

#[cfg(windows)]
fn main() -> Result<(), String> {
    velopack::VelopackApp::build()
        .on_after_install_fast_callback(|_| file_associations::register())
        .on_after_update_fast_callback(|_| file_associations::register())
        .on_before_uninstall_fast_callback(|_| file_associations::unregister())
        .set_auto_apply_on_startup(false)
        .run();
    elapsed_since_launch();
    if let single_instance::InstanceDisposition::Primary(guard) =
        single_instance::acquire_or_route()?
    {
        gpui_host::launch(guard)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The native NyatiDraw editor requires Windows.");
    std::process::exit(1);
}
