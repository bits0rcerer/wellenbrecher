#[cfg(not(feature = "simd_decoding"))]
use std::str::FromStr;

use io_uring::opcode;
use io_uring::squeue::PushError;
use io_uring::types::Fd;
use thiserror::Error;

use wellenbrecher_canvas::{Bgra, Canvas, CanvasError};

use crate::ring::SubmissionQueueSubmitter;
use crate::HELP_TEXT;

#[derive(Debug)]
pub enum Command {
    Help,
    Size,
    SetPixel { x: u32, y: u32, color: Bgra },
    GetPixel { x: u32, y: u32 },
}

impl TryFrom<&str> for Command {
    type Error = eyre::Report;

    #[cfg(feature = "simd_decoding")]
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() < 4 {
            return Err(eyre::eyre!("unknown command \"{value}\""));
        }

        let head = unsafe { (value.as_ptr() as *const u32).read_unaligned() };

        if head & simd_decoding::PX_VERB_MASK == simd_decoding::PX_VERB {
            let (_, args) = value.split_at(3);
            let mut splits = args.split(' ');

            let x = match splits.next() {
                Some(digits) if digits.len() <= simd_decoding::MAX_DECIMAL_STRING_LENGTH => {
                    simd_decoding::parse_u16_from_dec_str_simd(digits)?
                }
                Some(digits) => {
                    return Err(eyre::eyre!("x coordinate to out of range \"{digits}\""));
                }
                None => return Err(eyre::eyre!("invalid PX command \"{value}\"")),
            };

            let y = match splits.next() {
                Some(digits) if digits.len() <= simd_decoding::MAX_DECIMAL_STRING_LENGTH => {
                    simd_decoding::parse_u16_from_dec_str_simd(digits)?
                }
                Some(digits) => {
                    return Err(eyre::eyre!("y coordinate to out of range \"{digits}\""));
                }
                None => return Err(eyre::eyre!("invalid PX command \"{value}\"")),
            };

            return match splits.next() {
                None => Ok(Command::GetPixel { x, y }),
                Some(digits) => Ok(Command::SetPixel {
                    x,
                    y,
                    color: simd_decoding::parse_color_from_str_simd(digits)?,
                }),
            };
        }

        if head == simd_decoding::HELP_VERB {
            return Ok(Command::Help);
        }

        if head == simd_decoding::SIZE_VERB {
            return Ok(Command::Size);
        }

        Err(eyre::eyre!("invalid command \"{}\"", value))
    }

    #[cfg(not(feature = "simd_decoding"))]
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.as_bytes() {
            [b'H', b'E', b'L', b'P'] => Ok(Command::Help),
            [b'S', b'I', b'Z', b'E'] => Ok(Command::Size),
            [b'P', b'X', b' ', args @ ..] => {
                let mut args = unsafe { std::str::from_utf8_unchecked(args) }.split(' ');

                // argument x
                let x = match args.next() {
                    None => return Err(eyre::eyre!("invalid arguments for PX command")),
                    Some(x) => u32::from_str(x)?,
                };

                // argument y
                let y = match args.next() {
                    None => return Err(eyre::eyre!("invalid arguments for PX command")),
                    Some(y) => u32::from_str(y)?,
                };

                // command end or argument color
                let (color, c) = match args.next() {
                    None => return Ok(Command::GetPixel { x, y }),
                    Some(c) => (u32::from_str_radix(c, 16)?, c),
                };

                match (color, c.len()) {
                    (color, 2) => Ok(Command::SetPixel {
                        x,
                        y,
                        color: Bgra::from_bw(color as u8),
                    }),
                    (color, 6) => Ok(Command::SetPixel {
                        x,
                        y,
                        color: Bgra::from_rgb(color),
                    }),
                    (color, 8) => Ok(Command::SetPixel {
                        x,
                        y,
                        color: color.into(),
                    }),
                    (_, _) => Err(eyre::eyre!("color {} is invalid", c)),
                }
            }
            _ => Err(eyre::eyre!("unknown command \"{value}\"")),
        }
    }
}

impl Command {
    #[inline]
    pub fn handle_command<D, W: Fn(&mut io_uring::squeue::Entry, D)>(
        self,
        canvas: &mut Canvas,
        socket_fd: Fd,
        submitter: &mut SubmissionQueueSubmitter<D, W>,
        user_id: u32,
    ) -> Result<(), CommandExecutionError> {
        match self {
            Command::Help => {
                let write =
                    opcode::Write::new(socket_fd, HELP_TEXT.as_ptr(), HELP_TEXT.len() as u32)
                        .build()
                        .user_data(
                            crate::ring::pixel_flut_ring::UserData::write_buffer_drop(None).into(),
                        );

                unsafe {
                    submitter.push_raw(write)?;
                }
                Ok(())
            }
            Command::Size => {
                let msg = format!("SIZE {} {}\n", canvas.width(), canvas.height())
                    .into_boxed_str()
                    .into_boxed_bytes();
                let write = opcode::Write::new(socket_fd, msg.as_ptr(), msg.len() as u32)
                    .build()
                    .user_data(
                        crate::ring::pixel_flut_ring::UserData::write_buffer_drop(Some(msg)).into(),
                    );

                unsafe {
                    submitter.push_raw(write)?;
                }
                Ok(())
            }
            Command::SetPixel { x, y, color } => {
                canvas.set_pixel(x, y, color, user_id).map_err(|e| e.into())
            }
            Command::GetPixel { x, y } => {
                let color = u32::from(canvas.pixel(x, y).unwrap_or_default());
                let msg = format!("PX {x} {y} {color:0>8x}\n")
                    .into_boxed_str()
                    .into_boxed_bytes();
                let write = opcode::Write::new(socket_fd, msg.as_ptr(), msg.len() as u32)
                    .build()
                    .user_data(
                        crate::ring::pixel_flut_ring::UserData::write_buffer_drop(Some(msg)).into(),
                    );

                unsafe {
                    submitter.push_raw(write)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum CommandExecutionError {
    #[error("unable to submit: {0}")]
    Submission(#[from] PushError),
    #[error("invalid canvas operation {0}")]
    CanvasError(#[from] CanvasError),
}
