use rummelplatz::io_uring::opcode;
use rummelplatz::io_uring::squeue::PushError;
use rummelplatz::io_uring::types::Fd;
use rummelplatz::SubmissionQueueSubmitter;
use thiserror::Error;

use wellenbrecher_canvas::{Bgra, Canvas, CanvasError};

use crate::ring::write_buffer_drop::WriteBufferDropDescriptor;

#[derive(Debug)]
pub enum Command {
    Help,
    Size,
    SetPixel { x: u32, y: u32, color: Bgra },
    GetPixel { x: u32, y: u32 },
    Offset { x: u32, y: u32 },
}

#[derive(Copy, Clone, Default, Debug)]
pub struct StaticReplies {
    pub help: usize,
    pub size: usize,
}

impl Command {
    #[inline]
    pub fn handle_command<D, W: Fn(&mut rummelplatz::io_uring::squeue::Entry, D)>(
        self,
        canvas: &mut Canvas,
        socket_fd: Fd,
        submitter: &mut SubmissionQueueSubmitter<D, W>,
        static_replies: &mut StaticReplies,
        user_id: u32,
        user_offset: &mut (u32, u32),
    ) -> Result<(), CommandExecutionError> {
        match self {
            Command::Help => {
                static_replies.help += 1;
                Ok(())
            }
            Command::Size => {
                static_replies.size += 1;
                Ok(())
            }
            Command::SetPixel { x, y, color } => {
                let x = user_offset.0 + x;
                let y = user_offset.1 + y;
                canvas.set_pixel(x, y, color, user_id).map_err(|e| e.into())
            }
            Command::GetPixel { x, y } => {
                let x = user_offset.0 + x;
                let y = user_offset.1 + y;
                let color = u32::from(canvas.pixel(x, y).unwrap_or_default());
                let msg = format!("PX {x} {y} {color:0>8x}\n")
                    .into_boxed_str()
                    .into_boxed_bytes();
                let write = opcode::Write::new(socket_fd, msg.as_ptr(), msg.len() as u32)
                    .build()
                    .user_data(
                        crate::ring::pixel_flut_ring::UserData::write_buffer_drop(
                            WriteBufferDropDescriptor::Buffer(msg),
                        )
                        .into(),
                    );

                unsafe {
                    submitter.push_raw(write)?;
                }
                Ok(())
            }
            Command::Offset { x, y } => {
                if x >= canvas.width() || y >= canvas.height() {
                    return Err(CommandExecutionError::CanvasError(
                        CanvasError::PixelOutOfBounds { x, y },
                    ));
                }

                *user_offset = (x, y);
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
