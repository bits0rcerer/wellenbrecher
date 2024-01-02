use std::io;
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use rummelplatz::io_uring::opcode;
use rummelplatz::io_uring::squeue::Entry;
use rummelplatz::io_uring::types::Fd;
use rummelplatz::{ControlFlow, RingOperation, SubmissionQueueSubmitter};
use socket2::Socket;
use tracing::{info, warn};

use wellenbrecher_canvas::{Canvas, CanvasError};

use crate::ring::command::{CommandExecutionError, StaticReplies};
use crate::ring::command_ring::{CommandRing, CommandRingError};
use crate::ring::ring_coordination::UserState;
use crate::ring::write_buffer_drop::WriteBufferDropDescriptor;
use crate::{ring, HELP_TEXT};

#[derive(Debug)]
pub struct PixelflutConnectionHandler {
    canvas: Canvas,
    size_reply_buffer: Box<[u8]>,
}

impl PixelflutConnectionHandler {
    pub fn new(canvas: Canvas) -> Self {
        Self {
            size_reply_buffer: format!("SIZE {} {}\n", canvas.width(), canvas.height())
                .into_boxed_str()
                .into_boxed_bytes(),
            canvas,
        }
    }
}

impl RingOperation for PixelflutConnectionHandler {
    type RingData = Connection;
    type SetupError = eyre::Error;
    type TeardownError = eyre::Error;
    type ControlFlowWarn = eyre::Error;
    type ControlFlowError = eyre::Error;

    #[inline]
    fn setup<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        Ok(())
    }

    #[inline]
    fn on_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        completion_entry: rummelplatz::io_uring::cqueue::Entry,
        mut connection: Self::RingData,
        mut submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> (
        ControlFlow<Self::ControlFlowWarn, Self::ControlFlowError>,
        Option<Self::RingData>,
    ) {
        match completion_entry.result() {
            n if n > 0 => {
                unsafe {
                    connection.command_ring.advance_write_unchecked(n as usize);
                }

                /*
                To mitigate DoS attacks using commands that generate significantly more egress traffic
                than required ingress traffic, we only reply to the first occurrence of the
                    - HELP (>10x egress)
                    - SIZE (~ 2x egress)
                command that yield from one socket read.
                It should be ok to do that because:
                    - HELP/SIZE is only issued manually by non-machine players, that are not that fast.
                    - HELP/SIZE is only issued once for feature/canvas size detection by machines
                 */
                let mut static_replies = StaticReplies::default();
                loop {
                    match connection.command_ring.read_next_command() {
                        Ok(cmd) => match cmd.handle_command(
                            &mut self.canvas,
                            Fd(connection.socket.as_raw_fd()),
                            &mut submitter,
                            &mut static_replies,
                            connection.user_id,
                            &mut connection.user_offset,
                        ) {
                            Ok(()) => {}
                            Err(CommandExecutionError::CanvasError(
                                CanvasError::PixelOutOfBounds { x, y },
                            )) => {
                                warn!("[user: {}] tried to set pixel out of bounds: ({x}, {y}); closing connection…",connection.user_id);
                                drop(connection);
                                return (ControlFlow::Continue, None);
                            }
                            Err(e) => {
                                warn!("[user: {}] unable to execute command: {e}; closing connection…",connection.user_id);
                                drop(connection);
                                return (ControlFlow::Continue, None);
                            }
                        },
                        Err(CommandRingError::MoreDataRequired) => {
                            break;
                        }
                        Err(e) => {
                            warn!(
                                "[user: {}] error while parsing command: {e}; closing connection…",
                                connection.user_id
                            );
                            drop(connection);
                            return (ControlFlow::Continue, None);
                        }
                    }
                }
                unsafe {
                    let mut iovecs = Vec::with_capacity(0);
                    if static_replies.size > 0 {
                        if static_replies.size > 8 {
                            warn!("connection {} from {} might be trying to DoS using SIZE egress amplification",
                                connection.user_id, connection.address,
                            )
                        }

                        iovecs.push(libc::iovec {
                            iov_base: self.size_reply_buffer.as_ptr() as _,
                            iov_len: self.size_reply_buffer.len(),
                        })
                    }
                    if static_replies.help > 0 {
                        if static_replies.help > 8 {
                            warn!("connection {} from {} might be trying to DoS using HELP egress amplification",
                                connection.user_id, connection.address,
                            )
                        }
                        iovecs.push(libc::iovec {
                            iov_base: HELP_TEXT.as_ptr() as _,
                            iov_len: HELP_TEXT.len(),
                        })
                    }

                    if !iovecs.is_empty() {
                        let writev = opcode::Writev::new(
                            Fd(connection.socket.as_raw_fd()),
                            iovecs.as_ptr(),
                            iovecs.len() as u32,
                        )
                        .build()
                        .user_data(
                            ring::pixel_flut_ring::UserData::write_buffer_drop(
                                WriteBufferDropDescriptor::IoVec(iovecs),
                            )
                            .into(),
                        );
                        if let Err(e) = submitter.push_raw(writev) {
                            return (ControlFlow::Error(e.into()), None);
                        }
                    }
                }

                let (ptr, len) = connection.command_ring.contig_write();
                let read =
                    opcode::Read::new(Fd(RawFd::from(connection.socket.as_raw_fd())), ptr, len)
                        .build();

                match submitter.push(read, connection) {
                    Ok(()) => (ControlFlow::Continue, None),
                    Err(e) => (ControlFlow::Error(e.into()), None),
                }
            }
            0 => {
                drop(connection);
                (ControlFlow::Continue, None)
            }
            e => {
                let e = io::Error::from_raw_os_error(-e);
                warn!(
                    "unable to read from connection {}: {e}; closing connection…",
                    connection.address
                );
                (ControlFlow::Continue, None)
            }
        }
    }

    fn on_teardown_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _completion_entry: rummelplatz::io_uring::cqueue::Entry,
        connection: Self::RingData,
        _submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        drop(connection);
        Ok(())
    }
}

#[derive(Debug)]
pub struct Connection {
    pub user_id: u32,
    pub user_offset: (u32, u32),
    pub user_state: Arc<UserState>,
    pub socket: Socket,
    pub address: SocketAddr,
    pub command_ring: CommandRing,
}

impl Drop for Connection {
    fn drop(&mut self) {
        let connections = self.user_state.connections.fetch_sub(1, Ordering::Relaxed) - 1;
        info!(
            "- {} [user: {}, connections: {}]",
            self.address, self.user_id, connections,
        );
    }
}
