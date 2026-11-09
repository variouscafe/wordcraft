//! Keeps a finger's pointer events away from eframe, so one tap is one press (#mobile).
//!
//! eframe's web runner turns a finger into `egui::Event::PointerButton` twice: once from the
//! `pointerdown`/`pointerup` pair (it never looks at `pointerType`) and once from
//! `touchstart`/`touchend`. `preventDefault` on `touchstart` only stops the compatibility
//! *mouse* events, not the pointer events. egui then sees two presses and two releases at the
//! same spot in a row, counts a double-click, and the phone viewer's double-tap zoom fires on
//! every single tap (and undoes itself on the next). Source: eframe `src/web/events.rs` of the
//! pinned egui fork, `install_pointerdown` (canvas), `install_pointerup` (document),
//! `install_touchstart` (canvas), `install_touchend` (document).
//!
//! The fix is here, not in the fork: a capture-phase listener on `window` runs before any
//! target- or bubble-phase listener, and `install_event_handlers` puts eframe's pointer
//! listeners on the canvas and the document (both below `window`), so stopping a touch pointer
//! event at `window` means eframe's pointer handlers never run for it. The touch events keep
//! driving egui: press, release, `PointerGone`, and the `Touch` events the gestures read.
//!
//! What eframe's pointer handlers did besides the press, and where it goes now:
//! - `pointerdown` ran `runner.logic()` at once (Safari lets the clipboard be written only
//!   during an event callback). Not replaced: the touch handlers never did, and the app logic
//!   runs on the next frame. A Safari/iOS copy button may need the tap to be confirmed on a
//!   device.
//! - `pointerup` focused the canvas (iOS keyboard). Done here on the touch's `pointerup`;
//!   `touchend` still re-takes the text agent's focus after it, as before (that order is
//!   unchanged: pointerup, then touchend).
//!
//! Never touched: `mouse` and `pen` pointers (desktop and stylus behave as before), and
//! `preventDefault` (the browser's touch handling stays as it was).

/// Whether a pointer event is swallowed: a finger (`"touch"`) whose press started on the app
/// canvas. `started_on_canvas` is the event target for `pointerdown`, and for `pointerup` /
/// `pointercancel` whether this pointer's `pointerdown` was swallowed (the release goes to the
/// document under pointer capture, so its own target says little).
pub(crate) fn swallow(pointer_type: &str, started_on_canvas: bool) -> bool {
    pointer_type == "touch" && started_on_canvas
}

#[cfg(target_arch = "wasm32")]
pub(crate) use web::install;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;

    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::closure::Closure;

    use super::swallow;

    /// Installs the guard. Call before `eframe::WebRunner::start`, and keep the canvas the
    /// runner is given.
    pub(crate) fn install(canvas: &web_sys::HtmlCanvasElement) -> Result<(), String> {
        let window = web_sys::window().ok_or("no window")?;
        // Pointer ids whose `pointerdown` was swallowed: their `pointerup` / `pointercancel` is too.
        let held: Rc<RefCell<HashSet<i32>>> = Rc::default();
        for kind in ["pointerdown", "pointerup", "pointercancel"] {
            let held = held.clone();
            let canvas = canvas.clone();
            let handler = Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |event: web_sys::PointerEvent| {
                let id = event.pointer_id();
                let on_canvas = if kind == "pointerdown" {
                    event.target().is_some_and(|t| js_sys::Object::is(t.as_ref(), canvas.as_ref()))
                } else {
                    held.borrow_mut().remove(&id)
                };
                if !swallow(&event.pointer_type(), on_canvas) {
                    return;
                }
                if kind == "pointerdown" {
                    held.borrow_mut().insert(id);
                } else if kind == "pointerup" {
                    // What eframe's `pointerup` did for iOS' keyboard; `touchend` follows.
                    let options = web_sys::FocusOptions::new();
                    options.set_prevent_scroll(true);
                    let _ = canvas.focus_with_options(&options);
                }
                event.stop_immediate_propagation();
            });
            window.add_event_listener_with_callback_and_bool(kind, handler.as_ref().unchecked_ref(), true).map_err(|e| format!("{e:?}"))?;
            // The listeners live as long as the page.
            handler.forget();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::swallow;

    #[test]
    fn only_a_finger_on_the_canvas_is_swallowed() {
        assert!(swallow("touch", true));
        assert!(!swallow("touch", false), "a finger elsewhere on the page is left alone");
        assert!(!swallow("mouse", true), "the desktop mouse is never swallowed");
        assert!(!swallow("pen", true), "the stylus is never swallowed");
        assert!(!swallow("mouse", false));
        assert!(!swallow("", true), "an unknown pointer type is left alone");
    }
}
