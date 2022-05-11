// SPDX-License-Identifier: MPL-2.0-only

use std::cell::RefCell;
use std::rc::Rc;

use slog::Logger;
use smithay::utils::{Logical, Rectangle};

use super::{Popup, PopupRenderEvent};
#[derive(Debug, Clone)]
pub struct TopLevelSurface {
    pub(crate) s_top_level: Rc<RefCell<smithay::desktop::Window>>,
    pub(crate) dirty: bool,
    pub(crate) popups: Vec<Popup>,
    pub(crate) log: Logger,
    /// location offset of the window within the dock
    /// dimensions of the window in the dock
    pub(crate) rectangle: Rectangle<i32, Logical>,
    pub(crate) priority: u32,
    pub(crate) hidden: bool,
}

impl TopLevelSurface {
    /// Handles any events that have occurred since the last call, redrawing if needed.
    /// Returns true if the surface should be dropped.
    pub fn handle_events(&mut self) -> bool {
        if self.s_top_level.borrow().toplevel().get_surface().is_none() {
            return true;
        }
        // TODO replace with drain_filter when stable

        let mut i = 0;
        while i < self.popups.len() {
            let p = &mut self.popups[i];
            let should_keep = {
                if !p.s_surface.alive() {
                    false
                } else {
                    match p.next_render_event.take() {
                        Some(PopupRenderEvent::Closed) => false,
                        Some(PopupRenderEvent::Configure { width, height, .. }) => {
                            p.egl_surface.resize(width, height, 0, 0);
                            p.bbox.size = (width, height).into();
                            p.dirty = true;
                            true
                        }
                        Some(PopupRenderEvent::WaitConfigure) => {
                            p.next_render_event
                                .replace(Some(PopupRenderEvent::WaitConfigure));
                            true
                        }
                        None => true,
                    }
                }
            };

            if !should_keep {
                let _ = self.popups.remove(i);
            } else {
                i += 1;
            }
        }
        false
    }

    pub(crate) fn set_priority(&mut self, priority: u32) {
        self.priority = priority;
    }

    pub(crate) fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
    }
}

impl Drop for TopLevelSurface {
    fn drop(&mut self) {
        for p in &self.popups {
            p.c_popup.destroy();
            p.c_xdg_surface.destroy();
            p.c_surface.destroy();
        }
    }
}
