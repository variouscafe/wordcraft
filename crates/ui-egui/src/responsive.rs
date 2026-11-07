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

/// The reading page colour in effect on the surface shown (`view.readPageColor`, Read Mode's
/// own › View › Page Color): Read Mode's columns always take it, and so does the phone's page
/// — a Print-mode page read like a book, viewer or editor (#mobile). A desktop window's Print
/// page never does; that one keeps Word's Switch Modes.
pub fn reading_page_color(app: &crate::WordApp, ctx: &egui::Context) -> wordcraft_engine::cmd::view::ReadPageColor {
    let v = &app.session.view;
    if v.read_mode || is_phone_ctx(ctx) { v.read_page_color } else { wordcraft_engine::cmd::view::ReadPageColor::None }
}
