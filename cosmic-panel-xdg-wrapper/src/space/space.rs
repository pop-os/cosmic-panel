// SPDX-License-Identifier: MPL-2.0-only

use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    ffi::OsString,
    fs,
    os::unix::{net::UnixStream, prelude::AsRawFd},
    process::Child,
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::bail;
use itertools::{izip, Itertools};
use libc::c_int;

use super::{ClientEglSurface, Popup, PopupRenderEvent, ServerSurface, TopLevelSurface};
use crate::{
    shared_state::Focus,
    util::{exec_child, plugin_as_client_sock, smootherstep},
};
use cosmic_panel_config::config::{self, CosmicPanelConfig, WrapperConfig};
use sctk::{
    output::OutputInfo,
    reexports::{
        client::protocol::{wl_output as c_wl_output, wl_surface as c_wl_surface},
        client::{self, Attached, Main},
    },
    shm::AutoMemPool,
};
use slog::{info, trace, Logger};
use smithay::{
    backend::{
        egl::{
            context::{EGLContext, GlAttributes},
            display::EGLDisplay,
            ffi::{
                self,
                egl::{GetConfigAttrib, SwapInterval},
            },
            surface::EGLSurface,
        },
        renderer::{
            gles2::Gles2Renderer, utils::draw_surface_tree, Bind, Frame, ImportEgl, Renderer,
            Unbind,
        },
    },
    desktop::{
        utils::{damage_from_surface_tree, send_frames_surface_tree},
        Kind, PopupKind, PopupManager, Window,
    },
    nix::fcntl,
    reexports::{
        wayland_protocols::{
            wlr::unstable::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
            xdg_shell::client::{
                xdg_popup,
                xdg_positioner::{Anchor, Gravity, XdgPositioner},
                xdg_surface::{self, XdgSurface},
            },
        },
        wayland_server::{
            self, protocol::wl_surface::WlSurface as s_WlSurface, Client, Display as s_Display,
        },
    },
    utils::{Logical, Point, Rectangle, Size},
    wayland::{
        shell::xdg::{PopupSurface, PositionerState},
        SERIAL_COUNTER,
    },
};

#[derive(PartialEq, Copy, Clone, Debug)]
pub enum SpaceEvent {
    WaitConfigure {
        width: u32,
        height: u32,
    },
    Configure {
        width: u32,
        height: u32,
        serial: u32,
    },
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Visibility {
    Hidden,
    Visible,
    TransitionToHidden {
        last_instant: Instant,
        progress: Duration,
        prev_margin: i32,
    },
    TransitionToVisible {
        last_instant: Instant,
        progress: Duration,
        prev_margin: i32,
    },
}

impl Default for Visibility {
    fn default() -> Self {
        Self::Visible
    }
}


pub trait WrapperSpace {
    type Config: WrapperConfig;
    fn add_output(
        &mut self,
        output: Option<&c_wl_output::WlOutput>,
        output_info: Option<&OutputInfo>,
        pool: AutoMemPool,
        c_display: client::Display,
        layer_shell: Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        log: Logger,
        c_surface: Attached<c_wl_surface::WlSurface>,
        focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
    ) -> anyhow::Result<()>;
    fn bind_wl_display(&mut self, s_display: &s_Display) -> anyhow::Result<()>;
    fn update_pointer(&mut self, dim: (i32, i32));
    fn handle_button(&mut self, c_focused_surface: &c_wl_surface::WlSurface);
    fn add_top_level(&mut self, s_top_level: Rc<RefCell<Window>>);
    fn add_popup(
        &mut self,
        c_surface: c_wl_surface::WlSurface,
        c_xdg_surface: Main<XdgSurface>,
        s_surface: PopupSurface,
        parent: s_WlSurface,
        positioner: Main<XdgPositioner>,
        positioner_state: PositionerState,
        popup_manager: Rc<RefCell<PopupManager>>,
    );
    fn close_popups(&mut self);
    fn dirty_toplevel(&mut self, dirty_top_level_surface: &s_WlSurface, dim: Size<i32, Logical>);
    fn dirty_popup(&mut self, dirty_top_level_surface: &s_WlSurface, dirty_popup: PopupSurface);
    fn next_render_event(&self) -> Rc<Cell<Option<SpaceEvent>>>;
    fn reposition_popup(
        &mut self,
        popup: PopupSurface,
        positioner: Main<XdgPositioner>,
        positioner_state: PositionerState,
        token: u32,
    ) -> anyhow::Result<()>;
    fn server_surface_from_server_wl_surface(
        &self,
        active_surface: &s_WlSurface,
    ) -> Option<ServerSurface>;
    fn server_surface_from_client_wl_surface(
        &self,
        active_surface: &c_wl_surface::WlSurface,
    ) -> Option<ServerSurface>;
    fn handle_events(&mut self, time: u32, focus: &Focus) -> Instant;
    fn config(&self) -> Self::Config;
    fn spawn_clients(
        &mut self,
        display: &mut wayland_server::Display,
    ) -> anyhow::Result<Vec<(UnixStream, UnixStream)>>;
    fn visibility(&self) -> Visibility;
}


// TODO
// impl Drop for Space {
//     fn drop(&mut self) {
//         self.layer_surface.as_mut().map(|ls| ls.destroy());
//         self.layer_shell_wl_surface.as_mut().map(|wls| wls.destroy());
//     }
// }

#[derive(Debug)]
pub enum Alignment {
    Left,
    Center,
    Right,
}
