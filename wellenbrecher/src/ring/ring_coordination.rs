use std::ffi::CStr;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::ops::Sub;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use libc::c_int;
use rummelplatz::io_uring::opcode;
use rummelplatz::io_uring::squeue::{Entry, PushError};
use rummelplatz::io_uring::types::Fd;
use rummelplatz::{ControlFlow, RingOperation, SubmissionQueueSubmitter, IORING_CQE_F_MORE};
use socket2::Socket;
use tracing::{debug, error, info};

use crate::ring::command_ring::CommandRing;
use crate::ring::pixel_flut_ring::UserData;
use crate::ring::pixelflut_connection_handler::Connection;

#[derive(Debug)]
pub enum RingMessage {
    NewConnection,
    NewClient(NewClient),
    Signal(Box<libc::signalfd_siginfo>),
    Exit,
}

#[derive(Debug)]
pub struct NewClient {
    pub(crate) socket: Socket,
    pub(crate) address: SocketAddr,
    pub(crate) uid: u32,
    pub(crate) state: Arc<UserState>,
    pub(crate) buffer_size: usize,
}

#[derive(Debug)]
pub enum RingCoordination {
    Empress {
        sockets: Vec<Socket>,
        ring_fds: Vec<RawFd>,
        ring_fds_cycle_idx: usize,
        signal_fd: RawFd,

        connection_buffer_size: NonZeroUsize,
        clients: Arc<RwLock<Vec<Arc<UserState>>>>,
        ipv4_mask: Ipv4Addr,
        ipv6_mask: Ipv6Addr,

        last_exit_signal: Instant,
    },
    Lackey,
}

impl RingCoordination {
    pub fn lackey() -> Self {
        Self::Lackey
    }
    pub fn empress(
        sockets: Vec<Socket>,
        ring_fds: Vec<RawFd>,
        signal_fd: RawFd,
        connection_buffer_size: NonZeroUsize,
        clients: Arc<RwLock<Vec<Arc<UserState>>>>,
        ipv4_mask: Ipv4Addr,
        ipv6_mask: Ipv6Addr,
    ) -> Self {
        Self::Empress {
            sockets,
            ring_fds,
            ring_fds_cycle_idx: 0,
            signal_fd,
            connection_buffer_size,
            clients,
            ipv4_mask,
            ipv6_mask,
            last_exit_signal: Instant::now().sub(Duration::from_secs(20)),
        }
    }
}

impl RingOperation for RingCoordination {
    type RingData = RingMessage;
    type SetupError = eyre::Error;
    type TeardownError = eyre::Error;
    type ControlFlowWarn = eyre::Error;
    type ControlFlowError = eyre::Error;

    fn setup<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        mut submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        match self {
            RingCoordination::Empress {
                sockets, signal_fd, ..
            } => {
                for socket in sockets {
                    setup_socket(&mut submitter, &socket)?;
                }
                setup_signal(&mut submitter, *signal_fd)?;

                Ok(())
            }
            RingCoordination::Lackey => Ok(()),
        }
    }

    fn on_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        completion_entry: rummelplatz::io_uring::cqueue::Entry,
        ring_data: Self::RingData,
        mut submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> (
        ControlFlow<Self::ControlFlowWarn, Self::ControlFlowError>,
        Option<Self::RingData>,
    ) {
        match (ring_data, self) {
            (
                RingMessage::NewConnection,
                Self::Empress {
                    ring_fds,
                    ring_fds_cycle_idx,
                    clients,
                    ipv4_mask,
                    ipv6_mask,
                    connection_buffer_size,
                    ..
                },
            ) => {
                if completion_entry.result() < 0 {
                    let e = io::Error::from_raw_os_error(-completion_entry.result());
                    error!("failed to accept new client: {e}");
                    return (ControlFlow::Error(e.into()), None);
                }

                let socket = unsafe { Socket::from_raw_fd(completion_entry.result()) };

                let peer_addr = match socket.peer_addr() {
                    Ok(peer_addr) => peer_addr.as_socket().unwrap(),
                    Err(e) => {
                        debug!("connection lost early: {e}");
                        return (ControlFlow::Continue, Some(RingMessage::NewConnection));
                    }
                };

                let (user_id, user_state) = get_or_create_user_state(
                    clients
                        .write()
                        .expect("unable to acquire lock on clients")
                        .as_mut(),
                    peer_addr.ip(),
                    *ipv4_mask,
                    *ipv6_mask,
                );
                user_state.connections.fetch_add(1, Ordering::Relaxed);

                let new_client = NewClient {
                    socket,
                    address: peer_addr,
                    uid: user_id,
                    state: user_state,
                    buffer_size: connection_buffer_size.get(),
                };

                let fd = ring_fds.get(*ring_fds_cycle_idx % ring_fds.len()).unwrap();
                *ring_fds_cycle_idx = ring_fds_cycle_idx.wrapping_add(1);
                let msg = opcode::MsgRingData::new(
                    Fd(*fd),
                    0,
                    UserData::coordination(RingMessage::NewClient(new_client)).into(),
                    Some(IORING_CQE_F_MORE),
                )
                .build()
                .user_data(0);
                if let Err(e) = unsafe { submitter.push_raw(msg) } {
                    error!("unable to send new client to worker");
                    return (ControlFlow::Error(e.into()), None);
                }

                (ControlFlow::Continue, Some(RingMessage::NewConnection))
            }
            (
                RingMessage::Signal(signal),
                Self::Empress {
                    ring_fds,
                    last_exit_signal,
                    ..
                },
            ) => {
                let sig_name = unsafe {
                    CStr::from_ptr(libc::strsignal(signal.ssi_signo as c_int)).to_string_lossy()
                };

                match signal.ssi_signo as c_int {
                    libc::SIGINT | libc::SIGQUIT | libc::SIGTERM => {
                        if last_exit_signal.elapsed() < Duration::from_secs(10) {
                            info!("received another {sig_name} signal. Aborting...");
                            std::process::exit(-1);
                        }
                        *last_exit_signal = Instant::now();

                        info!("received {sig_name} signal. Shutting down...");

                        for fd in ring_fds {
                            let msg = opcode::MsgRingData::new(
                                Fd(fd.as_raw_fd()),
                                0,
                                UserData::coordination(RingMessage::Exit).into(),
                                Some(IORING_CQE_F_MORE),
                            )
                            .build()
                            .user_data(0);

                            if let Err(e) = unsafe { submitter.push_raw(msg) } {
                                error!("unable to shutdown gracefully: {e}\nAborting...");
                                std::process::exit(-1);
                            }
                        }
                    }
                    _ => {
                        info!("received {sig_name} signal. Ignoring...");
                    }
                }

                (ControlFlow::Continue, None)
            }
            (RingMessage::NewClient(new_client), _) => {
                info!(
                    "+ {} [user: {}, connections: {}]",
                    new_client.address,
                    new_client.uid,
                    new_client.state.connections.load(Ordering::Relaxed)
                );

                let connection = Connection {
                    user_id: new_client.uid,
                    user_offset: (0, 0),
                    user_state: new_client.state,
                    socket: new_client.socket,
                    address: new_client.address,
                    command_ring: CommandRing::new(new_client.buffer_size),
                };

                let (ptr, len) = connection.command_ring.contig_write();
                let read =
                    opcode::Read::new(Fd(RawFd::from(connection.socket.as_raw_fd())), ptr, len)
                        .build()
                        .user_data(UserData::pixelflut_connection_handler(connection).into());

                unsafe {
                    match submitter.push_raw(read) {
                        Ok(()) => (ControlFlow::Continue, None),
                        Err(e) => (ControlFlow::Error(e.into()), None),
                    }
                }
            }
            (RingMessage::Exit, _) => (ControlFlow::Exit, None),
            _ => unreachable!(),
        }
    }

    fn on_teardown_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _: rummelplatz::io_uring::cqueue::Entry,
        ring_data: Self::RingData,
        _: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        drop(ring_data);
        Ok(())
    }
}

fn setup_socket<W: Fn(&mut Entry, <RingCoordination as RingOperation>::RingData)>(
    submitter: &mut SubmissionQueueSubmitter<<RingCoordination as RingOperation>::RingData, W>,
    socket: &Socket,
) -> Result<(), PushError> {
    info!(
        "Listening on {}",
        socket.local_addr().unwrap().as_socket().unwrap()
    );
    let accept_multi = opcode::AcceptMulti::new(Fd(socket.as_raw_fd())).build();
    submitter.push(accept_multi, RingMessage::NewConnection)
}

fn setup_signal<W: Fn(&mut Entry, <RingCoordination as RingOperation>::RingData)>(
    submitter: &mut SubmissionQueueSubmitter<<RingCoordination as RingOperation>::RingData, W>,
    signal_fd: RawFd,
) -> Result<(), PushError> {
    let (siginfo_ptr, user_data) = unsafe {
        let mut siginfo = Box::new_zeroed();
        (
            siginfo.as_mut_ptr(),
            RingMessage::Signal(siginfo.assume_init()),
        )
    };
    let read = opcode::Read::new(
        Fd(signal_fd),
        siginfo_ptr as *mut _,
        std::mem::size_of::<libc::signalfd_siginfo>() as u32,
    )
    .build();
    submitter.push(read, user_data)
}

#[derive(Debug)]
pub struct UserState {
    pub(crate) ip: IpAddr,
    pub(crate) connections: AtomicUsize,
}

pub(crate) fn get_or_create_user_state(
    clients: &mut Vec<Arc<UserState>>,
    ip: IpAddr,
    ipv4_mask: Ipv4Addr,
    ipv6_mask: Ipv6Addr,
) -> (u32, Arc<UserState>) {
    let ip = match ip {
        IpAddr::V4(ip) => {
            let ip = ip.octets();
            let mask = ipv4_mask.octets();
            IpAddr::from([
                ip[0] & mask[0],
                ip[1] & mask[1],
                ip[2] & mask[2],
                ip[3] & mask[3],
            ])
        }
        IpAddr::V6(ip) => {
            let ip = ip.octets();
            let mask = ipv6_mask.octets();
            IpAddr::from([
                ip[0] & mask[0],
                ip[1] & mask[1],
                ip[2] & mask[2],
                ip[3] & mask[3],
                ip[4] & mask[4],
                ip[5] & mask[5],
                ip[6] & mask[6],
                ip[7] & mask[7],
                ip[8] & mask[8],
                ip[9] & mask[9],
                ip[10] & mask[10],
                ip[11] & mask[11],
                ip[12] & mask[12],
                ip[13] & mask[13],
                ip[14] & mask[14],
                ip[15] & mask[15],
            ])
        }
    };

    if let Some((idx, state)) = clients.iter().enumerate().find(|(_, state)| state.ip == ip) {
        return ((idx + 1) as u32, state.clone());
    }

    let new_state = Arc::new(UserState {
        ip,
        connections: Default::default(),
    });

    // re-use old entry
    if let Some((idx, state)) = clients
        .iter_mut()
        .enumerate()
        .find(|(_, state)| state.connections.load(Ordering::Relaxed) == 0)
    {
        *state = new_state.clone();
        return ((idx + 1) as u32, new_state);
    }

    // create new entry
    clients.push(new_state.clone());
    (clients.len() as u32, new_state)
}
