use super::Space;
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Display;
use std::{process::Child, time::Instant};

#[derive(Default, Debug)]
pub struct SpaceManager {
    pub(crate) spaces: Vec<Space>,
    active: Option<usize>,
}

impl SpaceManager {
    pub fn push_space(&mut self, s: Space) {
        if self.spaces.len() == 0 {
            self.active = Some(0);
        }
        self.spaces.push(s);
    }

    pub fn active_space(&mut self) -> Option<&mut Space> {
        self.active
            .and_then(|active_i| self.spaces.get_mut(active_i))
    }

    pub fn remove_space_with_output(&mut self, output_name: &str) {
        self.spaces.retain(|s| s.output.1.name == output_name)
    }

    pub fn update_active(&mut self, active_surface: Option<WlSurface>) {
        if let Some(active_space) = self
            .active
            .and_then(|active_i| self.spaces.get_mut(active_i))
        {
            if Some(&*active_space.layer_shell_wl_surface) == active_surface.as_ref() {
                return;
            }
        }

        if let Some(space) = self.active_space() {
            space.close_popups()
        }
        if let Some(active_surface) = active_surface {
            // set new active space if possible
            if let Some(active_space_i) = self
                .spaces
                .iter_mut()
                .position(|s| *s.layer_shell_wl_surface == active_surface)
            {
                self.active = Some(active_space_i);
            }
        } else {
            self.active = None;
        }
    }

    pub fn apply_display(&mut self, s_display: &Display) {
        for space in &mut self.spaces {
            space.apply_display(s_display);
        }
    }

    pub fn handle_events(&mut self, time: Instant, children: &mut Vec<Child>) -> Instant {
        self.spaces
            .iter_mut()
            .map(|space| space.handle_events(time.elapsed().as_millis() as u32, children))
            .fold(time.clone(), |max_t, t| t.max(max_t))
    }
}
