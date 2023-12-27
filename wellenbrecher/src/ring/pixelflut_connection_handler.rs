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
use tracing::{error, info, warn};

use wellenbrecher_canvas::{Canvas, CanvasError};

use crate::ring::command::CommandExecutionError;
use crate::ring::command_ring::{CommandRing, CommandRingError};
use crate::ring::ring_coordination::UserState;

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

                loop {
                    match connection.command_ring.read_next_command() {
                        Ok(cmd) => match cmd.handle_command(
                            &mut self.canvas,
                            Fd(connection.socket.as_raw_fd()),
                            &mut submitter,
                            connection.user_id,
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
                error!(
                    "unable to read from connection {}: {e}; closing connection…",
                    connection.address
                );
                (
                    ControlFlow::Warn(eyre::eyre!(
                        "unable to read from connection {}: {e}",
                        connection.address
                    )),
                    None,
                )
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
