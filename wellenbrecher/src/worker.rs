use std::io;
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use io_uring::types::{CancelBuilder, Fd};
use io_uring::{cqueue, opcode, squeue, IoUring};
use os_pipe::PipeReader;
use socket2::Socket;
use tracing::{debug, error, trace};

use wellenbrecher_canvas::{Bgra, Canvas, CanvasError};

use crate::cli::Args;
use crate::command::CommandExecutionError;
use crate::command_ring::{CommandRing, CommandRingError};
use crate::UserState;

pub enum WorkerMessage {
    NewClient(NewClientMessage),
    Exit,
}

pub struct NewClientMessage {
    pub(crate) socket_fd: RawFd,
    pub(crate) address: SocketAddr,
    pub(crate) uid: u32,
    pub(crate) state: Arc<UserState>,
    pub(crate) buffer_size: usize,
}

fn new_io_uring(ring_size: u32) -> io::Result<IoUring<squeue::Entry, cqueue::Entry>> {
    IoUring::builder()
        .setup_single_issuer()
        .setup_coop_taskrun()
        .setup_defer_taskrun()
        .build(ring_size)
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

#[derive(Debug)]
pub enum WorkerRingUserData {
    Cancel,
    Exit,
    WorkerMessage,
    Read(Connection),
    Write(Option<Vec<u8>>),
}

impl From<WorkerRingUserData> for u64 {
    fn from(value: WorkerRingUserData) -> u64 {
        unsafe { std::mem::transmute(Box::new(value)) }
    }
}

impl From<Box<WorkerRingUserData>> for u64 {
    fn from(value: Box<WorkerRingUserData>) -> u64 {
        unsafe { std::mem::transmute(value) }
    }
}

impl WorkerRingUserData {
    unsafe fn from_raw(user_data: u64) -> Box<Self> {
        std::mem::transmute(user_data)
    }
}

macro_rules! submit_read_to_ring {
    ($worker_id:ident, $ring_submission:ident, $connection:ident) => {
        unsafe {
            let (ptr, len) = $connection.command_ring.contig_write();

            let read = opcode::Read::new(Fd(RawFd::from($connection.socket.as_raw_fd())), ptr, len)
                .build()
                .user_data(WorkerRingUserData::Read($connection).into());
            let result = $ring_submission.push(&read);
            if let Err(e) = &result {
                error!(
                    "failed to submit \"opcode::Read\" to ring on worker {}: {e}. \
                This will crash this thread and leak all its connections. :(",
                    $worker_id
                );
            }
            result?
        }
    };
}

pub fn worker(rx: PipeReader, args: Args, index: usize) -> eyre::Result<()> {
    let mut canvas = Canvas::open(
        args.canvas_file_link.as_ref(),
        args.keep_canvas_file_link,
        args.width.get(),
        args.height.get(),
        Bgra::default(),
    )?;

    let mut ring = new_io_uring(args.io_uring_size.get())?;
    let (ring_submitter, mut ring_submission, mut ring_completion) = ring.split();

    let mut pipe_buffer = [0u8; std::mem::size_of::<*const NewClientMessage>()];
    unsafe {
        let read = opcode::Read::new(
            Fd(rx.as_raw_fd()),
            pipe_buffer.as_mut_ptr(),
            pipe_buffer.len() as u32,
        )
        .build()
        .user_data(WorkerRingUserData::WorkerMessage.into());

        ring_submission.push(&read).unwrap();
        ring_submission.sync();
    }

    'ring_loop: loop {
        ring_submission.sync();
        ring_submitter.submit_and_wait(1)?;

        ring_completion.sync();
        'completion_loop: for c in ring_completion.by_ref() {
            let user_data = unsafe { WorkerRingUserData::from_raw(c.user_data()) };
            match *user_data {
                WorkerRingUserData::Exit => {
                    break 'ring_loop;
                }
                WorkerRingUserData::Write(buffer) => {
                    drop(buffer);
                }
                WorkerRingUserData::WorkerMessage => {
                    if c.result() < 0 {
                        debug!("failed to read from pipe: {}", c.result());
                        break 'ring_loop;
                    }

                    let msg: Box<WorkerMessage> =
                        unsafe { Box::from_raw(u64::from_be_bytes(pipe_buffer) as *mut _) };

                    match *msg {
                        WorkerMessage::NewClient(msg) => {
                            trace!(
                                "[user: {}] received connection {} on worker {}",
                                msg.uid,
                                msg.address,
                                index,
                            );

                            let socket = unsafe { Socket::from_raw_fd(msg.socket_fd) };
                            let connection = Connection {
                                user_id: msg.uid,
                                user_state: msg.state,
                                socket,
                                address: msg.address,
                                command_ring: CommandRing::new(msg.buffer_size),
                            };

                            submit_read_to_ring!(index, ring_submission, connection);
                        }
                        WorkerMessage::Exit => {
                            debug!("[worker-{index}] shutting down...");
                            break 'ring_loop;
                        }
                    }

                    unsafe {
                        let read = opcode::Read::new(
                            Fd(rx.as_raw_fd()),
                            pipe_buffer.as_mut_ptr(),
                            pipe_buffer.len() as u32,
                        )
                        .build()
                        .user_data(WorkerRingUserData::WorkerMessage.into());

                        ring_submission.push(&read).unwrap();
                    }
                }
                WorkerRingUserData::Read(mut connection) => {
                    match c.result() {
                        n if n > 0 => {
                            unsafe {
                                connection.command_ring.advance_write_unchecked(n as usize);
                            }

                            loop {
                                match connection.command_ring.read_next_command() {
                                    Ok(cmd) => match cmd.handle_command(
                                        &mut canvas,
                                        Fd(connection.socket.as_raw_fd()),
                                        &mut ring_submission,
                                        connection.user_id,
                                    ) {
                                        Ok(_) => {}
                                        Err(CommandExecutionError::CanvasError(
                                            CanvasError::PixelOutOfBounds { x, y },
                                        )) => {
                                            error!("[user: {}] tried to set pixel out of bounds: ({x}, {y}); closing connection…",connection.user_id);
                                            continue 'completion_loop;
                                        }
                                        Err(e) => {
                                            error!("[user: {}] unable to execute command: {e}; closing connection…",connection.user_id);
                                            continue 'completion_loop;
                                        }
                                    },
                                    Err(CommandRingError::MoreDataRequired) => {
                                        break;
                                    }
                                    Err(e) => {
                                        error!("[user: {}] error while parsing command: {e}; closing connection…",connection.user_id);
                                        continue 'completion_loop;
                                    }
                                }
                            }

                            submit_read_to_ring!(index, ring_submission, connection)
                        }
                        0 => {
                            // connection closed
                            continue;
                        }
                        e => {
                            let e = io::Error::from_raw_os_error(e);
                            error!(
                                "unable to read from connection {}: {e}",
                                connection.socket.peer_addr().unwrap().as_socket().unwrap()
                            );
                            continue;
                        }
                    }
                }
                WorkerRingUserData::Cancel => unreachable!(),
            }
        }
    }

    unsafe {
        let cancel = opcode::AsyncCancel2::new(CancelBuilder::any())
            .build()
            .user_data(WorkerRingUserData::Cancel.into());
        ring_submission.push(&cancel)?;
    }

    'cancel_loop: loop {
        ring_submission.sync();
        ring_submitter.submit_and_wait(1)?;
        ring_completion.sync();

        for c in ring_completion.by_ref() {
            let user_data = unsafe { WorkerRingUserData::from_raw(c.user_data()) };
            match *user_data {
                WorkerRingUserData::Exit => {}
                WorkerRingUserData::WorkerMessage => {}
                WorkerRingUserData::Read(connection) => drop(connection),
                WorkerRingUserData::Write(buffer) => drop(buffer),

                WorkerRingUserData::Cancel => break 'cancel_loop,
            }
        }
    }

    debug!("[worker-{index}] finished");
    Ok(())
}
