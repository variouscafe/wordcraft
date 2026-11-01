//! The phone breakpoint the narrow passes share (the Backstage's icon rail, the compact status
//! bar). Below it the UI rearranges for touch; desktop windows (≥760 pt) never cross it, so
//! their rendering is unchanged.

/// The widest a viewport still counts as a phone (pt).
pub const PHONE_MAX_W: f32 = 600.0;

/// Whether this frame's viewport is phone-width.
pub fn is_phone(ui: &egui::Ui) -> bool {
    is_phone_ctx(ui.ctx())
}

/// [`is_phone`] where only the context is at hand.
pub fn is_phone_ctx(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport_rect().width()) < PHONE_MAX_W
}

/// Whether the page reads as a viewer this frame: phone-width, and the title bar's Edit button
/// hasn't switched it to editing (`CanvasState::phone_editing`). Touching the page then never
/// focuses it, so the soft keyboard stays down and the text stays readable — Word's phone app
/// opens the same way. Selecting (long-press, grips, the context menu) and every ribbon command
/// still work; only the keyboard is held back.
pub fn touch_reading(app: &crate::WordApp, ctx: &egui::Context) -> bool {
    is_phone_ctx(ctx) && !app.canvas.phone_editing
}
