// SPDX-License-Identifier: MPL-2.0-only

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use slog::Logger;
use smithay::utils::{Logical, Point};

use super::{Popup, PopupRenderEvent};

#[derive(PartialEq, Copy, Clone, Debug)]
pub enum RenderEvent {
    WaitConfigure,
    Configure { width: u32, height: u32 },
    Closed,
}

#[derive(PartialEq, Copy, Clone, Debug, Eq)]
pub enum ActiveState {
    InactiveCleared(bool),
    ActiveFullyRendered(bool),
}

impl PartialOrd for ActiveState {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ActiveState {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (ActiveState::InactiveCleared(_), ActiveState::InactiveCleared(_)) => Ordering::Equal,
            (ActiveState::InactiveCleared(_), ActiveState::ActiveFullyRendered(_)) => {
                Ordering::Less
            }
            (ActiveState::ActiveFullyRendered(_), ActiveState::InactiveCleared(_)) => {
                Ordering::Greater
            }
            (ActiveState::ActiveFullyRendered(_), ActiveState::ActiveFullyRendered(_)) => {
                Ordering::Equal
            }
        }
    }
}
#[derive(Debug, Clone)]
pub struct TopLevelSurface {
    pub s_top_level: Rc<RefCell<smithay::desktop::Window>>,
    pub dirty: bool,
    pub dimensions: (u32, u32),
    pub popups: Vec<Popup>,
    pub log: Logger,
    pub active: ActiveState,
    pub loc_offset: Point<i32, Logical>,
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
