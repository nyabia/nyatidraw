//! Platform-neutral geometry contract between DOM chrome and the native canvas.
//! Windows owns the COM/HWND integration in `windows`; future hosts implement
//! the same visible canvas / UI input regions without leaking OS types upstream.

#[cfg(windows)]
pub(crate) mod windows;

#[derive(Clone, Debug)]
pub(crate) struct HostLayout {
    /// Webview-client CSS coordinates: x, y, width, height, device scale.
    pub canvas: [f64; 5],
    /// Top-level interactive UI rectangles overlapping the canvas, in CSS px.
    pub ui_regions: Vec<[f64; 4]>,
}

impl HostLayout {
    pub fn valid(&self) -> bool {
        let [_, _, width, height, scale] = self.canvas;
        self.canvas.iter().all(|value| value.is_finite())
            && width > 0.0
            && height > 0.0
            && scale > 0.0
            && self.ui_regions.len() <= 128
            && self.ui_regions.iter().all(|rect| {
                rect.iter().all(|value| value.is_finite()) && rect[2] >= 0.0 && rect[3] >= 0.0
            })
    }
}
