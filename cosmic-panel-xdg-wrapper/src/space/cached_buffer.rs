// SPDX-License-Identifier: MPL-2.0-only

use anyhow::{bail, Result};
use sctk::{
    reexports::client::{
        protocol::{
            wl_buffer::{self, WlBuffer},
            wl_shm,
            wl_shm_pool::WlShmPool,
            wl_surface::WlSurface,
        },
        Attached, Main,
    },
    shm,
};
use slog::{trace, warn, Logger};
use smithay::{
    backend::renderer::{buffer_type, BufferType},
    wayland::{
        compositor::BufferAssignment,
        shm::{with_buffer_contents, BufferData},
    },
};
use std::{
    cell::Cell,
    fs::File,
    io::{BufWriter, Write},
    os::unix::io::AsRawFd,
    rc::Rc,
};
use tempfile::tempfile;

#[derive(Debug)]
pub struct CachedBuffers {
    buffers: Vec<Buffer>,
    log: Logger,
}

// FIXME Cursor images are broken
impl CachedBuffers {
    pub fn new(log: Logger) -> Self {
        Self {
            buffers: Default::default(),
            log,
        }
    }

    pub fn write_and_attach_buffer(
        &mut self,
        buffer_assignment: &BufferAssignment,
        surface: &WlSurface,
        shm: &Attached<wl_shm::WlShm>,
    ) -> Result<()> {
        if let BufferAssignment::NewBuffer {
            buffer: source_buffer,
            ..
        } = buffer_assignment
        {
            trace!(self.log, "checking buffer format...");
            if let Some(BufferType::Shm) = buffer_type(source_buffer) {
                with_buffer_contents(
                    source_buffer,
                    move |slice: &[u8], buffer_metadata: BufferData| {
                        if let Some(format) = shm::Format::from_raw(buffer_metadata.format.to_raw())
                        {
                            let pos = self.index_of_pair_buffer(
                                buffer_metadata.width,
                                buffer_metadata.height,
                                format,
                                shm,
                            );
                            trace!(
                                self.log,
                                "getting index of buffer for writing... {:?}",
                                &pos
                            );
                            self.buffers[pos].try_write_buffer_and_attach(
                                slice,
                                buffer_metadata,
                                surface,
                            )
                        } else {
                            bail!("unsupported format!")
                        }
                    },
                )?
            } else {
                bail!("not an shm buffer...")
            }
        } else {
            warn!(
                self.log,
                "Buffer was removed, not going to attempt to update buffer..."
            );
            Ok(())
        }
    }

    fn index_of_pair_buffer(
        &mut self,
        x: i32,
        y: i32,
        format: shm::Format,
        shm: &Attached<wl_shm::WlShm>,
    ) -> usize {
        let mut best_candidate = None;
        for (i, buffer) in self.buffers.iter().enumerate() {
            if buffer.free() {
                if buffer.x == x && buffer.y == y && buffer.format == format {
                    return i;
                }
                // TODO
                // if let Some((_, best_size)) = best_candidate {
                //     if x * y > buffer.pool_size && best_size < buffer.pool_size {
                //         best_candidate = Some((i, buffer.pool_size));
                //     } else if x * y < buffer.pool_size && buffer.pool_size < best_size {
                //         best_candidate = Some((i, buffer.pool_size));
                //     }
                // } else {
                //     best_candidate = Some((i, buffer.pool_size));
                // }
                best_candidate = Some((i, buffer.pool_size));

            }
        }
        trace!(
            self.log,
            "best candidate for buffer({}, {}): {:?}",
            x,
            y,
            best_candidate
        );
        if let Some((i, _)) = best_candidate {
            return i;
        }
        // need to create new pool / buffer and use that instead
        let new_buffer = Buffer::new(shm, x, y, format, self.log.clone());
        self.buffers.push(new_buffer);
        self.buffers.len() - 1
    }
}

#[derive(Debug)]
pub struct Buffer {
    free: Rc<Cell<bool>>,
    file: File,
    pool: Main<WlShmPool>,
    pool_size: i32,
    buffer: Main<WlBuffer>,
    x: i32,
    y: i32,
    format: wl_shm::Format,
    log: Logger,
}

impl Buffer {
    pub fn new(
        shm: &Attached<wl_shm::WlShm>,
        x: i32,
        y: i32,
        format: wl_shm::Format,
        log: Logger,
    ) -> Self {
        let file = tempfile().expect("Unable to create a tempfile");
        let size = x * y * 4;
        let pool = shm.create_pool(
            file.as_raw_fd(),
            size, // size in bytes of the shared memory (4 bytes / pixel)
        );
        let buffer = pool.create_buffer(0, x, y, x * 4, format);
        let free = Rc::new(Cell::new(true));
        let captured_free = free.clone();
        // mark buffer as free once it is released by the server
        let logger = log.clone();
        buffer.quick_assign(move |_self, event, _dispatch_data| {
            if let wl_buffer::Event::Release = event {
                trace!(
                    logger,
                    "marking buffer as taken until released by the server"
                );
                captured_free.replace(true);
            }
        });
        Self {
            free,
            file,
            pool,
            buffer,
            pool_size: size,
            x: x,
            y: y,
            format,
            log,
        }
    }

    pub fn try_write_buffer_and_attach(
        &mut self,
        source: &[u8],
        buffer_metadata: BufferData,
        surface: &WlSurface,
    ) -> Result<()> {
        // resize pool and buffer if necessary
        if self.x != buffer_metadata.width && self.y != buffer_metadata.height {
            trace!(self.log, "resizing pool and buffer before write and attach");
            self.x = buffer_metadata.width;
            self.y = buffer_metadata.height;
            self.buffer.destroy();
            let required_size = self.x * self.y * 4;
            if self.pool_size < required_size {
                self.pool_size = required_size;
                self.pool.resize(self.pool_size);
            }

            if let Some(format) = shm::Format::from_raw(buffer_metadata.format.to_raw()) {
                self.buffer = self
                    .pool
                    .create_buffer(0, self.x, self.y, self.x * 4, format);
            } else {
                bail!("bad buffer format!")
            }
        }

        trace!(self.log, "writing to selected buffer");
        let mut writer = BufWriter::new(&mut self.file);
        writer.write_all(source)?;
        writer.flush()?;

        trace!(self.log, "attaching buffer to surface");
        surface.attach(Some(&self.buffer), self.x, self.y);
        surface.damage(0, 0, self.x, self.y);
        surface.commit();

        trace!(
            self.log,
            "marking buffer as taken until released by the server"
        );
        self.free.replace(false);
        Ok(())
    }

    pub fn free(&self) -> bool {
        self.free.get()
    }
}
