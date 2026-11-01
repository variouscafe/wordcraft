//! The phone breakpoint the narrow passes share (the Backstage's icon rail, the compact status
//! bar). Below it the UI rearranges for touch; desktop windows (≥760 pt) never cross it, so
//! their rendering is unchanged.

/// The widest a viewport still counts as a phone (pt).
pub const PHONE_MAX_W: f32 = 600.0;

/// Whether this frame's viewport is phone-width.
pub fn is_phone(ui: &egui::Ui) -> bool {
    ui.ctx().input(|i| i.viewport_rect().width()) < PHONE_MAX_W
}
