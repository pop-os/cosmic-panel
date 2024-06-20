// SPDX-License-Identifier: MPL-2.0

use std::{
    io::{BufWriter, Write},
    os::unix::net::UnixStream,
    sync::Arc,
};

use smithay::reexports::wayland_server::{self, Client};
// SPDX-License-Identifier: MPL-2.0
use anyhow::{bail, Result};
use sctk::{
    reexports::client::protocol::{wl_shm, wl_surface::WlSurface},
    shm::multi::MultiPool,
};
use smithay::{
    backend::renderer::{buffer_type, BufferType},
    wayland::{
        compositor::BufferAssignment,
        shm::{with_buffer_contents, BufferData},
    },
};

use crate::client_state::WrapperClientCompositorState;

use super::WrapperSpace;

/// utility function which maps a value [0, 1] -> [0, 1] using the smootherstep function
pub fn smootherstep(t: f32) -> f32 {
    (6.0 * t.powi(5) - 15.0 * t.powi(4) + 10.0 * t.powi(3)).clamp(0.0, 1.0)
}

/// helper function for inserting a wrapped applet client
pub fn get_client_sock(display: &mut wayland_server::DisplayHandle) -> (Client, UnixStream) {
    let (display_sock, client_sock) = UnixStream::pair().unwrap();

    (
        display
            .insert_client(
                display_sock,
                Arc::new(WrapperClientCompositorState { compositor_state: Default::default() }),
            )
            .unwrap(),
        client_sock,
    )
}

pub(crate) fn write_and_attach_buffer(    buffer_assignment: &BufferAssignment,
    cursor_surface: &WlSurface,
    multipool_ctr: usize,
    multipool: &mut MultiPool<(WlSurface, usize)>,
) -> Result<()> {
    if let BufferAssignment::NewBuffer(source_buffer) = buffer_assignment {
        if let Some(BufferType::Shm) = buffer_type(source_buffer) {
            with_buffer_contents(
                source_buffer,
                move |from: *const u8, length: usize, buffer_metadata: BufferData| {
                    if let Ok(format) = wl_shm::Format::try_from(buffer_metadata.format as u32) {
                        let BufferData { offset, width, height, stride, .. } = buffer_metadata;
                        let Ok((_, buff, to)) = multipool.create_buffer(
                            width,
                            stride,
                            height,
                            &(cursor_surface.clone(), multipool_ctr),
                            format,
                        ) else {
                            bail!("Failed to create buffer");
                        };

                        let mut writer = BufWriter::new(to);
                        let from: &[u8] = unsafe { std::slice::from_raw_parts(from, length) };
                        let offset: usize = offset.try_into()?;
                        let height: usize = height.try_into()?;
                        let stride: usize = stride.try_into()?;

                        writer.write_all(&from[offset..(offset + height * stride)])?;
                        writer.flush()?;

                        cursor_surface.attach(Some(buff), 0, 0);
                        cursor_surface.damage(0, 0, width, height as i32);
                        cursor_surface.commit();

                        Ok(())
                    } else {
                        bail!("Unsupported format!")
                    }
                },
            )?
        } else {
            bail!("Not an shm buffer")
        }
    } else {
        bail!("Missing new buffer.")
    }
}
