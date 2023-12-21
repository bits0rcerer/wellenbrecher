use std::fmt::{Debug, Formatter};
use std::path::Path;
use std::ptr::{slice_from_raw_parts, slice_from_raw_parts_mut};

use bytemuck_derive::{Pod, Zeroable};
use shared_memory::{Shmem, ShmemError};
use thiserror::Error;
use tracing::error;

#[derive(Debug, Clone, Copy, Pod, Zeroable, Eq, PartialEq)]
#[repr(C)]
pub struct Bgra {
    pub b: u8,
    pub g: u8,
    pub r: u8,
    pub a: u8,
}

impl From<u32> for Bgra {
    #[inline]
    fn from(value: u32) -> Self {
        Self {
            b: ((value & 0xff000000) >> 24) as u8,
            g: ((value & 0x00ff0000) >> 16) as u8,
            r: ((value & 0x0000ff00) >> 8) as u8,
            a: (value & 0x000000ff) as u8,
        }
    }
}

impl From<Bgra> for u32 {
    #[inline]
    fn from(value: Bgra) -> u32 {
        (value.b as u32) << 24 | (value.g as u32) << 16 | (value.r as u32) << 8 | (value.a as u32)
    }
}

impl Default for Bgra {
    #[inline]
    fn default() -> Self {
        Bgra::from(0u32)
    }
}

impl Bgra {
    #[inline]
    pub fn from_rgba(value: u32) -> Self {
        Self {
            r: ((value & 0xff000000) >> 24) as u8,
            g: ((value & 0x00ff0000) >> 16) as u8,
            b: ((value & 0x0000ff00) >> 8) as u8,
            a: (value & 0x000000ff) as u8,
        }
    }

    #[inline]
    pub fn from_argb(value: u32) -> Self {
        Self {
            a: ((value & 0xff000000) >> 24) as u8,
            r: ((value & 0x00ff0000) >> 16) as u8,
            g: ((value & 0x0000ff00) >> 8) as u8,
            b: (value & 0x000000ff) as u8,
        }
    }

    #[inline]
    pub fn from_bw(bw: u8) -> Self {
        Self {
            r: bw,
            g: bw,
            b: bw,
            a: 255,
        }
    }

    #[inline]
    pub fn from_rgb(rgb: u32) -> Self {
        Self {
            r: ((rgb & 0xff0000) >> 16) as u8,
            g: ((rgb & 0x00ff00) >> 8) as u8,
            b: (rgb & 0x0000ff) as u8,
            a: 0xff,
        }
    }

    #[inline]
    pub fn rgb(&self) -> u32 {
        (self.r as u32) << 16 | (self.g as u32) << 8 | (self.b as u32)
    }
}

pub type UserID = u32;

pub struct Canvas {
    width: u32,
    height: u32,
    len: usize,
    #[allow(dead_code)]
    shared_memory: Shmem,
    data: *mut Bgra,
    user_id_map: *mut UserID,
}

unsafe impl Send for Canvas {}

impl Debug for Canvas {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(
                f,
                r"Canvas: {{
    width: {},
    height: {},
    shared_memory: {},
    shared_memory_owner: {},
}}",
                self.width,
                self.height,
                self.shared_memory.get_os_id(),
                self.shared_memory.is_owner()
            )
        } else {
            write!(
                f,
                r"Canvas: {{ width: {}, height: {}, shared_memory: {}, shared_memory_owner: {} }}",
                self.width,
                self.height,
                self.shared_memory.get_os_id(),
                self.shared_memory.is_owner()
            )
        }
    }
}

impl Canvas {
    #[tracing::instrument]
    pub fn open(
        canvas_path: &Path,
        persistent_canvas: bool,
        width: u32,
        height: u32,
        fill: Bgra,
    ) -> Result<Self, CanvasError> {
        let canvas_size = (width * height) as usize * std::mem::size_of::<Bgra>();
        let uid_map_size = (width * height) as usize * std::mem::size_of::<UserID>();
        let size = canvas_size + uid_map_size;

        let shared_memory = shared_memory::ShmemConf::new()
            .size(size)
            .flink(canvas_path)
            .create()
            .map(|m| {
                unsafe {
                    (*slice_from_raw_parts_mut(m.as_ptr() as *mut Bgra, (width * height) as usize))
                        .fill(fill);
                }
                m
            });
        let shared_memory = match shared_memory {
            Ok(m) => Ok(m),
            Err(ShmemError::LinkExists) => shared_memory::ShmemConf::new()
                .size(size)
                .flink(canvas_path)
                .open(),
            Err(e) => Err(e),
        };
        let mut shared_memory = match shared_memory {
            Ok(m) => m,
            Err(e) => {
                error!("unable to open or create canvas {:?}: {e}", canvas_path);
                return Err(e.into());
            }
        };

        // shared memory will be destroyed when owner is dropped
        // in case we want to keep the canvas around, we disown ourself
        shared_memory.set_owner(shared_memory.is_owner() && !persistent_canvas);

        Ok(Canvas {
            width,
            height,
            len: (width * height) as usize,
            data: shared_memory.as_ptr() as *mut _,
            user_id_map: unsafe { shared_memory.as_ptr().add(canvas_size) } as *mut _,
            shared_memory,
        })
    }

    #[inline]
    fn coords_to_index(&self, x: u32, y: u32) -> usize {
        (y * self.width + x) as usize
    }

    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> Result<Bgra, CanvasError> {
        let idx = self.coords_to_index(x, y);
        if idx >= self.len {
            return Err(CanvasError::PixelOutOfBounds { x, y });
        }
        unsafe { Ok(std::ptr::read(self.data.add(idx))) }
    }

    #[inline]
    pub fn set_pixel(&self, x: u32, y: u32, color: Bgra, user_id: u32) -> Result<(), CanvasError> {
        let idx = self.coords_to_index(x, y);
        if idx >= self.len {
            return Err(CanvasError::PixelOutOfBounds { x, y });
        }

        match color.a {
            0 => Ok(()),
            255 => {
                unsafe { self.data.add(idx).write(color) };
                unsafe { self.user_id_map.add(idx).write(user_id) };
                Ok(())
            }
            alpha => {
                let color1 = self.pixel(x, y).unwrap().rgb();
                let color2 = color.rgb();
                let alpha = alpha as u32;

                let mut rb = color1 & 0xff00ff;
                let mut g = color1 & 0x00ff00;
                rb += (((color2 & 0xff00ff).saturating_sub(rb)) * alpha) >> 8;
                g += (((color2 & 0x00ff00).saturating_sub(g)) * alpha) >> 8;
                let new_color = Bgra::from_rgb((rb & 0xff00ff) | (g & 0xff00));
                unsafe { self.data.add(idx).write(new_color) };
                unsafe { self.user_id_map.add(idx).write(user_id) };
                Ok(())
            }
        }
    }

    #[inline]
    pub fn pixel_slice(&self) -> &[Bgra] {
        unsafe { &*slice_from_raw_parts(self.data, self.len) }
    }

    #[inline]
    pub fn pixel_byte_slice(&self) -> &[u8] {
        unsafe {
            &*slice_from_raw_parts(
                self.data as *const _,
                self.len * std::mem::size_of::<Bgra>(),
            )
        }
    }

    #[inline]
    pub fn user_id_slice(&self) -> &[UserID] {
        unsafe { &*slice_from_raw_parts(self.user_id_map, self.len) }
    }

    #[inline]
    pub fn user_id_byte_slice(&self) -> &[u8] {
        unsafe {
            &*slice_from_raw_parts(
                self.user_id_map as *const _,
                self.len * std::mem::size_of::<UserID>(),
            )
        }
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }
}

#[derive(Debug, Error)]
pub enum CanvasError {
    #[error("pixel ({x}, {y}) out of bounds")]
    PixelOutOfBounds { x: u32, y: u32 },
    #[error("mapping error: {0}")]
    Mapping(#[from] ShmemError),
}
