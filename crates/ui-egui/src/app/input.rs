#[cfg(not(target_arch = "wasm32"))]
use super::i18n;
use super::{WordApp, json};

impl WordApp {
    /// Read a Control-click as the right-click on macOS, then offer at most one synthetic gesture
    /// step to the host. eframe carries an unconsumed RawInput into the next pass and into hidden
    /// logic ticks, so offering it again would duplicate input. The step stays queued until a
    /// final (not discarded) UI pass acknowledges it, and a press/release pair stays intact.
    pub fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Capture the surface from the raw input itself. `InputState::viewport_rect` and the
        // last painted screen rect are stale in a hidden `run_logic` tick; RawInput is the host's
        // current answer, including minimized/occluded and a Wayland `inner_rect: None`.
        if raw.viewport_id == egui::ViewportId::ROOT
            && let Some(geometry) = crate::control::SurfaceGeometry::from_raw(raw, ctx)
        {
            // A hidden resize and change back still invalidate an older capture, even if
            // neither intermediate geometry reaches a drawing or logic pass.
            if self.surface_geometry != Some(geometry) {
                self.capture_epoch = self.capture_epoch.wrapping_add(1);
            }
            self.surface_geometry = Some(geometry);
        }
        if cfg!(target_os = "macos") {
            control_click_as_secondary(&mut raw.events, &mut self.control_click);
        }
        if self.synthetic_pending || self.synthetic.is_empty() || self.fonts_frames < 2 || !self.control_input_is_roots(ctx) {
            return;
        }
        let Some((events, n)) = self.peek_synthetic() else { return };
        if events.iter().any(|e| matches!(e, egui::Event::Key { .. } | egui::Event::Text(_))) {
            // Control input belongs to this window's remembered widget even while another app
            // is active. Give only this input frame egui focus, without requesting OS activation.
            raw.focused = true;
            if let (Some(ctx), Some((id, _))) = (&self.ctx, self.canvas.widget_id) {
                // Canvas keys run before painting updates last frame's native focus flag.
                self.canvas.focused = ctx.memory(|m| m.has_focus(id));
            }
        }
        raw.events.extend(events);
        self.synthetic_pending = true;
        self.synthetic_injected = n;
    }

    /// The control channel's next input frame, taken off the queue.
    pub(crate) fn next_synthetic(&mut self) -> Option<Vec<egui::Event>> {
        let (events, n) = self.peek_synthetic()?;
        self.synthetic.drain(..n);
        Some(events)
    }

    /// The control channel's next input frame: one pointer event (after a move to it), or keys
    /// and text up to the next key release; with how many queued events it takes.
    fn peek_synthetic(&self) -> Option<(Vec<egui::Event>, usize)> {
        if self.synthetic.is_empty() {
            return None;
        }
        let n = match self.synthetic.first() {
            Some(egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. }) => 1,
            _ => self.synthetic.iter().position(|e| matches!(e, egui::Event::Key { pressed: false, .. })).map_or(self.synthetic.len(), |i| i + 1),
        };
        let mut events = Vec::new();
        if let Some(egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. }) = self.synthetic.first() {
            events.push(egui::Event::PointerMoved(*p));
        }
        let n = n.min(self.synthetic.len());
        events.extend(self.synthetic.iter().take(n).cloned());
        Some((events, n))
    }

    /// Raw input read in the known UI phase, first pass only: dropped files and the SmartArt Text
    /// Pane's Tab/Shift+Tab, before the font warm-up return. `logic` also runs hidden, where its
    /// input events are from the previous shown frame.
    pub(super) fn handle_ui_input_events(&mut self, ctx: &egui::Context) {
        // eframe does not preserve dropped files for a later frame, so a drop in the first two
        // passes would otherwise disappear.
        #[cfg(not(target_arch = "wasm32"))]
        if ctx.current_pass_index() == 0 {
            let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
            if !dropped.is_empty() {
                self.open_dropped_files(dropped);
                let lang = i18n::Lang::from_pref(&self.ui.language);
                self.install_ui_fonts(ctx, lang);
            }
        }
        // SmartArt Text Pane (#2701): Tab demotes and Shift+Tab promote the focused item, as
        // Word's pane does. This is an input action, so it runs in the first UI pass only; hidden
        // `run_logic` ticks still see the previous shown frame's keys.
        if ctx.current_pass_index() == 0 && self.session.view.smart_art_pane && self.smart_art_row_focused && self.smart_art_item.is_some() {
            let shift_tab = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
            let tab = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
            if shift_tab || tab {
                ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
                let _ = self.run(if shift_tab { "smartArt.promote" } else { "smartArt.demote" }, json!({"index": self.smart_art_item.unwrap_or(0)}));
            }
        }
    }

    /// A click on the window's chrome (ribbon tabs and empty ribbon space, galleries, check
    /// boxes, the status bar, panes) hands keyboard focus back to the document, as in Word, so the
    /// caret keeps blinking and typing continues where it was (#1093). egui drops the page's focus
    /// on any click elsewhere; only a click that left nothing focused counts, so a text field
    /// (font size, zoom, command search) keeps the focus it took, and clicks on a dialog, menu or
    /// popup, or while one is open, leave focus alone. Clicks on the page are the canvas's own.
    pub(super) fn refocus_after_chrome_click(&mut self, ctx: &egui::Context) {
        if self.ui.backstage || self.dialog.is_some() || self.colors.is_some() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.file_browser.open.is_some() {
            return;
        }
        // Phone-width reading (#mobile): the page is a viewer, so a click on the chrome must not
        // hand the keyboard to it — the soft keyboard would cover the text being read.
        if crate::responsive::touch_reading(self, ctx) {
            return;
        }
        let Some(pos) = ctx.input(|i| if i.pointer.any_click() { i.pointer.interact_pos() } else { None }) else { return };
        // The ribbon dropped over the page in Focus (#1678) is chrome, though it lies over the page.
        let on_focus_ribbon = self.focus_ribbon && ctx.layer_id_at(pos).is_some_and(|l| l.id == egui::Id::unique("focus_ribbon"));
        // The page handles its own clicks (comment balloons there take the keyboard themselves).
        if !on_focus_ribbon && (self.canvas.balloon.is_some() || self.canvas.viewport.is_some_and(|v| v.contains(pos))) {
            return;
        }
        let on_chrome = on_focus_ribbon || ctx.layer_id_at(pos).is_none_or(|l| l.order == egui::Order::Background);
        if on_chrome && ctx.memory(|m| m.focused().is_none()) && !egui::Popup::is_any_open(ctx) {
            self.canvas.want_focus = true;
            // The page has already been drawn, so it only sees `want_focus` next frame, and its own
            // `focused` flag (which gates the keys) isn't recomputed until then either. Claim both
            // now, or the click swallows the first keystroke.
            self.canvas.focused = true;
            // Only a page this pass drew. The File page's back arrow closes the File page in the
            // pass that drew it instead of the page, and focus on a widget missing from the pass
            // names a node AccessKit doesn't have: with accessibility on, AccessKit ends the program
            // over that. `want_focus` hands the keyboard over next frame instead.
            if let Some((id, pass)) = self.canvas.widget_id
                && pass == ctx.cumulative_pass_nr()
            {
                ctx.memory_mut(|m| m.request_focus(id));
            }
            // The canvas reads keys before it takes focus back (#1107), so it counts as focused
            // now: a keystroke in the very next frame reaches the page instead of being dropped.
            self.canvas.focused = true;
            ctx.request_repaint();
        }
    }
}

/// macOS reads a Control-click as the right-click (winit reports the primary button with Control
/// held), so menus open on it as on a two-finger click. The release follows its press even if
/// Control was let go in between.
pub(crate) fn control_click_as_secondary(events: &mut [egui::Event], held: &mut bool) {
    for e in events {
        if let egui::Event::PointerButton { button: button @ egui::PointerButton::Primary, pressed, modifiers, .. } = e {
            if *pressed {
                *held = modifiers.ctrl && !modifiers.mac_cmd;
            }
            if *held {
                *button = egui::PointerButton::Secondary;
            }
            if !*pressed {
                *held = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Pos2};

    /// egui applies a `PointerButton`'s own position to `interact_pos`, so a touch-style
    /// press — which arrives with no preceding `PointerMoved` — must still hit-test and click
    /// where it landed. These pin that, for the sequences eframe's web backend delivers
    /// (review of #5050 questioned it; these are the proof it holds).
    #[test]
    fn a_press_without_any_preceding_move_still_clicks_a_button() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(390.0, 844.0)).build_ui_state(
            |ui: &mut egui::Ui, clicked: &mut bool| {
                *clicked |= ui.button("Tap me").clicked();
            },
            false,
        );
        h.run();
        let rect = h.get_by_role_and_label(egui::accesskit::Role::Button, "Tap me").rect();
        let pos = Pos2::new(rect.center().x, rect.center().y);
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
            h.run();
        }
        assert!(*h.state(), "a touch-style press and release with no move must click");
    }

    /// The control for the test above: the same sequence with a preceding move (what
    /// `ensure_presses_are_preceded_by_moves` inserts, and what kittest's own click does).
    #[test]
    fn a_press_with_a_preceding_move_clicks_a_button() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(390.0, 844.0)).build_ui_state(
            |ui: &mut egui::Ui, clicked: &mut bool| {
                *clicked |= ui.button("Tap me").clicked();
            },
            false,
        );
        h.run();
        let rect = h.get_by_role_and_label(egui::accesskit::Role::Button, "Tap me").rect();
        let pos = Pos2::new(rect.center().x, rect.center().y);
        h.event(Event::PointerMoved(pos));
        h.run();
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
            h.run();
        }
        assert!(*h.state(), "with a preceding move the click must fire");
    }

    /// Sanity: kittest's own click() must click (it hover-moves first, then presses).
    #[test]
    fn kittest_own_click_clicks_a_button() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(390.0, 844.0)).build_ui_state(
            |ui: &mut egui::Ui, clicked: &mut bool| {
                *clicked |= ui.button("Tap me").clicked();
            },
            false,
        );
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::Button, "Tap me").click();
        h.run();
        assert!(*h.state(), "kittest's own click must work in this harness");
    }

    /// The exact web batching: eframe-web can deliver a touch press and its release in ONE
    /// pass (no repaint ran between touchstart and touchend), with no move at all.
    #[test]
    fn a_press_and_release_in_one_pass_without_a_move_clicks_a_button() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(390.0, 844.0)).build_ui_state(
            |ui: &mut egui::Ui, clicked: &mut bool| {
                *clicked |= ui.button("Tap me").clicked();
            },
            false,
        );
        h.run();
        let rect = h.get_by_role_and_label(egui::accesskit::Role::Button, "Tap me").rect();
        let pos = Pos2::new(rect.center().x, rect.center().y);
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        }
        h.run();
        assert!(*h.state(), "a same-pass press+release with no move must click");
    }
}
