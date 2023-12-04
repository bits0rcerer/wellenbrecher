use std::io;
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use io_uring::opcode;
use io_uring::squeue::Entry;
use io_uring::types::Fd;
use socket2::Socket;
use tracing::{debug, error, warn};

use wellenbrecher_canvas::{Canvas, CanvasError};

use crate::ring::command::CommandExecutionError;
use crate::ring::command_ring::{CommandRing, CommandRingError};
use crate::ring::{ControlFlow, RingOperation, SubmissionQueueSubmitter};
use crate::UserState;

#[derive(Debug)]
pub struct PixelflutConnectionHandler {
    canvas: Canvas,
}

impl PixelflutConnectionHandler {
    pub fn new(canvas: Canvas) -> Self {
        Self { canvas }
    }
}

impl RingOperation for PixelflutConnectionHandler {
    type RingData = Connection;

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
        completion_entry: io_uring::cqueue::Entry,
        mut connection: Self::RingData,
        mut submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> ControlFlow {
        match completion_entry.result() {
            n if n > 0 => {
                unsafe {
                    connection.command_ring.advance_write_unchecked(n as usize);
                }

                loop {
                    match connection.command_ring.read_next_command() {
                        Ok(cmd) => match cmd.handle_command(
                            &mut self.canvas,
                            Fd(connection.socket.as_raw_fd()),
                            &mut submitter,
                            connection.user_id,
                        ) {
                            Ok(_) => {}
                            Err(CommandExecutionError::CanvasError(
                                CanvasError::PixelOutOfBounds { x, y },
                            )) => {
                                warn!("[user: {}] tried to set pixel out of bounds: ({x}, {y}); closing connection…",connection.user_id);
                                return ControlFlow::Warn(eyre::eyre!("[user: {}] tried to set pixel out of bounds: ({x}, {y}); closing connection…",connection.user_id));
                            }
                            Err(e) => {
                                warn!("[user: {}] unable to execute command: {e}; closing connection…",connection.user_id);
                                return ControlFlow::Warn(eyre::eyre!("[user: {}] unable to execute command: {e}; closing connection…",connection.user_id));
                            }
                        },
                        Err(CommandRingError::MoreDataRequired) => {
                            break;
                        }
                        Err(e) => {
                            error!(
                                "[user: {}] error while parsing command: {e}; closing connection…",
                                connection.user_id
                            );
                            return ControlFlow::Warn(eyre::eyre!(
                                "[user: {}] error while parsing command: {e}; closing connection…",
                                connection.user_id
                            ));
                        }
                    }
                }

                let (ptr, len) = connection.command_ring.contig_write();
                let read =
                    opcode::Read::new(Fd(RawFd::from(connection.socket.as_raw_fd())), ptr, len)
                        .build();

                match submitter.push(read, connection) {
                    Ok(()) => ControlFlow::Continue,
                    Err(e) => ControlFlow::Error(e.into()),
                }
            }
            0 => ControlFlow::Continue,
            e => {
                let e = io::Error::from_raw_os_error(e);
                error!("unable to read from connection {}: {e}", connection.address);
                ControlFlow::Error(eyre::eyre!(
                    "unable to read from connection {}: {e}",
                    connection.address
                ))
            }
        }
    }

    fn on_teardown_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _completion_entry: io_uring::cqueue::Entry,
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
    pub user_state: Arc<UserState>,
    pub socket: Socket,
    pub address: SocketAddr,
    pub command_ring: CommandRing,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.user_state.connections.fetch_sub(1, Ordering::Relaxed);

        debug!(
            "[user: {}] connection from {} dropped",
            self.user_id, self.address,
        );
    }
}
