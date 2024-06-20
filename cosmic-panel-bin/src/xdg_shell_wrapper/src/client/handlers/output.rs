// SPDX-License-Identifier: MPL-2.0

use sctk::{
    output::{Mode as c_Mode, OutputHandler, OutputInfo, OutputState},
    reexports::{
        client::protocol::wl_output::Subpixel as c_Subpixel,
        client::{protocol::wl_output, Connection, QueueHandle},
    },
};
use smithay::{
    output::{Mode as s_Mode, Output, PhysicalProperties, Scale, Subpixel as s_Subpixel},
    reexports::wayland_server::{backend::GlobalId, DisplayHandle},
    utils::Transform,
};
use tracing::{error, info, warn};
use xdg_shell_wrapper_config::WrapperConfig;

use crate::xdg_shell_wrapper::{
    client_state::ClientState, server_state::ServerState, shared_state::GlobalState,
    space::WrapperSpace,
};

impl OutputHandler for GlobalState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.client_state.output_state
    }

    fn new_output(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let info = match self.output_state().info(&output) {
            Some(info) if info.name.is_some() => info,
            _ => return,
        };

        let GlobalState {
            client_state:
                ClientState {
                    compositor_state,
                    layer_state,
                    viewporter_state,
                    fractional_scaling_manager,
                    ..
                },
            server_state: ServerState { display_handle, .. },
            space,
            ..
        } = self;

        let config = space.config();
        let configured_outputs = match config.outputs() {
            xdg_shell_wrapper_config::WrapperOutput::All => info.name.iter().cloned().collect(),
            xdg_shell_wrapper_config::WrapperOutput::Name(list) => list,
        };

        if configured_outputs.iter().any(|configured| Some(configured) == info.name.as_ref()) {
            // construct a surface for an output if possible
            let s_output = c_output_as_s_output(display_handle, &info);

            self.client_state.outputs.push((output.clone(), s_output.0.clone(), s_output.1));
            if let Err(err) = space.new_output(
                compositor_state,
                fractional_scaling_manager.as_ref(),
                viewporter_state.as_ref(),
                layer_state,
                conn,
                qh,
                Some(output),
                Some(s_output.0),
                Some(info),
            ) {
                warn!("{}", err);
            }
        }
    }

    fn update_output(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let info = match self.output_state().info(&output) {
            Some(info) if info.name.is_some() => info,
            _ => return,
        };

        let GlobalState {
            client_state:
                ClientState {
                    compositor_state,
                    layer_state,
                    fractional_scaling_manager,
                    viewporter_state,
                    ..
                },
            server_state: ServerState { display_handle, .. },
            space,
            ..
        } = self;

        let config = space.config();
        let configured_outputs = match config.outputs() {
            xdg_shell_wrapper_config::WrapperOutput::All => info.name.iter().cloned().collect(),
            xdg_shell_wrapper_config::WrapperOutput::Name(list) => list,
        };

        if configured_outputs.iter().any(|configured| Some(configured) == info.name.as_ref()) {
            if let Some(saved_output) = self.client_state.outputs.iter_mut().find(|o| o.0 == output)
            {
                let res = space.update_output(output.clone(), saved_output.1.clone(), info.clone());
                if let Err(err) = res {
                    error!("{}", err);
                } else if matches!(res, Ok(false)) {
                    let s_output = c_output_as_s_output(display_handle, &info);

                    if let Err(err) = space.new_output(
                        compositor_state,
                        fractional_scaling_manager.as_ref(),
                        viewporter_state.as_ref(),
                        layer_state,
                        conn,
                        qh,
                        Some(output),
                        Some(s_output.0),
                        Some(info),
                    ) {
                        warn!("{}", err);
                    }
                }
            }
        }
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        info!("output destroyed {:?}", &output);
        let info = match self.output_state().info(&output) {
            Some(info) if info.name.is_some() => info,
            _ => return,
        };
        let GlobalState { space, .. } = self;

        let config = space.config();
        let configured_outputs = match config.outputs() {
            xdg_shell_wrapper_config::WrapperOutput::All => info.name.iter().cloned().collect(),
            xdg_shell_wrapper_config::WrapperOutput::Name(list) => list,
        };
        info!("output destroyed {:?}", info.name.as_ref());

        info!("configured outputs {:?}", &configured_outputs);

        if configured_outputs.iter().any(|configured| Some(configured) == info.name.as_ref()) {
            if let Some(saved_output) = self.client_state.outputs.iter().position(|o| o.0 == output)
            {
                let (c, s, _) = self.client_state.outputs.remove(saved_output);
                if let Err(err) = space.output_leave(c, s) {
                    warn!("{}", err);
                }
            }
        }
    }
}

/// convert client output to server output
pub fn c_output_as_s_output(    dh: &DisplayHandle,
    info: &OutputInfo,
) -> (Output, GlobalId) {
    let s_output = Output::new(
        info.name.clone().unwrap_or_default(), // the name of this output,
        PhysicalProperties {
            size: info.physical_size.into(), // dimensions (width, height) in mm
            subpixel: match info.subpixel {
                c_Subpixel::None => s_Subpixel::None,
                c_Subpixel::HorizontalRgb => s_Subpixel::HorizontalRgb,
                c_Subpixel::HorizontalBgr => s_Subpixel::HorizontalBgr,
                c_Subpixel::VerticalRgb => s_Subpixel::VerticalRgb,
                c_Subpixel::VerticalBgr => s_Subpixel::VerticalBgr,
                _ => s_Subpixel::Unknown,
            }, // subpixel information
            make: info.make.clone(),         // make of the monitor
            model: info.model.clone(),       // model of the monitor
        },
    );
    for c_Mode { dimensions, refresh_rate, current, preferred } in &info.modes {
        let s_mode = s_Mode { size: (*dimensions).into(), refresh: *refresh_rate };
        if *preferred {
            s_output.set_preferred(s_mode);
        }
        if *current {
            s_output.change_current_state(
                Some(s_mode),
                Some(Transform::Normal),
                Some(Scale::Integer(info.scale_factor)),
                Some(info.location.into()),
            )
        }
        s_output.add_mode(s_mode);
    }
    let s_output_global = s_output.create_global::<GlobalState>(dh);
    (s_output, s_output_global)
}
