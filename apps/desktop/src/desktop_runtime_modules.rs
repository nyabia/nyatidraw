mod artwork_clipboard;
#[cfg(windows)]
mod desktop_canvas;
mod edit_gesture;
mod edit_worker;
#[cfg(windows)]
mod file_associations;
mod layout_store;
mod live_ink;
mod native_canvas;
mod performance;
mod performance_probe;
mod performance_workload;
mod preview;
#[cfg(windows)]
mod recent_projects;
#[cfg(windows)]
mod render_host;
mod save_as;
mod scaled_export;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod sketchbook;
mod sprite_export;
mod transform_gesture;
mod transform_worker;
#[cfg(windows)]
mod updates;
mod workspace_appearance;

#[cfg(windows)]
mod image_import;
mod input_preferences;
mod pressure_preferences;
