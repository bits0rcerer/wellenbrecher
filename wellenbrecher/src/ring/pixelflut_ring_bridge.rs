use std::io::Write;
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::sync::Arc;

use io_uring::opcode;
use io_uring::squeue::Entry;
use io_uring::types::Fd;
use os_pipe::{PipeReader, PipeWriter};
use socket2::Socket;
use tokio::io::AsyncWriteExt;
use tracing::{debug, instrument, trace};

use crate::ring::command_ring::CommandRing;
use crate::ring::pixelflut_connection_handler::Connection;
use crate::ring::{ControlFlow, RingOperation, SubmissionQueueSubmitter};
use crate::UserState;

#[derive(Debug)]
pub enum WorkerMessage {
    NewClient(NewClientMessage),
    Exit,
}

#[derive(Debug)]
pub struct NewClientMessage {
    pub(crate) socket_fd: RawFd,
    pub(crate) address: SocketAddr,
    pub(crate) uid: u32,
    pub(crate) state: Arc<UserState>,
    pub(crate) buffer_size: usize,
}

pub struct Sender {
    tx: PipeWriter,
}

impl Sender {
    pub fn signal_exit(&mut self) -> eyre::Result<()> {
        let msg = Box::new(WorkerMessage::Exit);
        let raw = Box::into_raw(msg);
        self.tx.write_all(&(raw as u64).to_be_bytes())?;

        Ok(())
    }

    pub async fn signal_new_client(&mut self, msg: NewClientMessage) -> eyre::Result<()> {
        let msg = Box::new(WorkerMessage::NewClient(msg));
        let raw = Box::into_raw(msg);

        let mut tx = unsafe { tokio_pipe::PipeWrite::from_raw_fd(self.tx.as_raw_fd()) };
        tx.write_u64(raw as u64).await?;

        _ = tx.into_raw_fd();
        Ok(())
    }
}

#[derive(Debug)]
pub struct Receiver {
    rx: PipeReader,
    buffer: [u8; std::mem::size_of::<*const WorkerMessage>()],
}

impl RingOperation for Receiver {
    type RingData = ();

    #[inline]
    #[instrument(skip(submitter))]
    fn setup<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        mut submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        let read = opcode::Read::new(
            Fd(self.rx.as_raw_fd()),
            self.buffer.as_mut_ptr(),
            self.buffer.len() as u32,
        )
        .build();
        submitter.push(read, ())?;
        Ok(())
    }

    #[inline]
    #[instrument(skip(submitter))]
    fn on_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        completion_entry: io_uring::cqueue::Entry,
        _: Self::RingData,
        mut submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> ControlFlow {
        if completion_entry.result() < 0 {
            debug!("failed to read from pipe: {}", completion_entry.result());
            return ControlFlow::Error(eyre::eyre!(
                "failed to read from pipe: {}",
                completion_entry.result()
            ));
        }

        let msg: Box<WorkerMessage> =
            unsafe { Box::from_raw(u64::from_be_bytes(self.buffer) as *mut _) };

        let flow = match *msg {
            WorkerMessage::NewClient(msg) => {
                trace!("received connection {} on worker {}", msg.uid, msg.address,);

                let socket = unsafe { Socket::from_raw_fd(msg.socket_fd) };
                let connection = Connection {
                    user_id: msg.uid,
                    user_state: msg.state,
                    socket,
                    address: msg.address,
                    command_ring: CommandRing::new(msg.buffer_size),
                };

                let (ptr, len) = connection.command_ring.contig_write();
                let read =
                    opcode::Read::new(Fd(RawFd::from(connection.socket.as_raw_fd())), ptr, len)
                        .build()
                        .user_data(
                            crate::ring::pixel_flut_ring::UserData::pixelflut_connection_handler(
                                connection,
                            )
                            .into(),
                        );

                unsafe {
                    match submitter.push_raw(read) {
                        Ok(()) => ControlFlow::Continue,
                        Err(e) => ControlFlow::Error(e.into()),
                    }
                }
            }
            WorkerMessage::Exit => ControlFlow::Exit,
        };

        match self.setup(submitter) {
            Ok(()) => flow,
            Err(e) => ControlFlow::Error(e),
        }
    }

    fn on_teardown_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _: io_uring::cqueue::Entry,
        _: Self::RingData,
        _: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        Ok(())
    }
}

pub fn new() -> eyre::Result<(Sender, Receiver)> {
    let (rx, tx) = os_pipe::pipe().expect("unable to create pipe");
    Ok((
        Sender { tx },
        Receiver {
            rx,
            buffer: Default::default(),
        },
    ))
}
