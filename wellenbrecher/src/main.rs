#![feature(vec_into_raw_parts)]

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::{NonZeroU32, NonZeroUsize};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::raw::c_int;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use clap::Parser;
use nftables::helper::NftablesError;
use os_pipe::PipeWriter;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::EnvFilter;

use crate::cli::Args;
use crate::firewall::ConnectionLimit;
use crate::worker::{worker, NewClientMessage, WorkerMessage};

mod cli;
mod command;
mod command_ring;
mod firewall;
mod worker;

const HELP_TEXT: &[u8] = br#"Welcome to Pixelflut!

Commands:
    HELP                -> get this information page
    SIZE                -> get the size of the canvas
    PX <x> <y>          -> get the color of pixel (x, y)
    PX <x> <y> <COLOR>  -> set the color of pixel (x, y)

    COLOR:
        Grayscale: ww          ("00"       black .. "ff"       white)
        RGB:       rrggbb      ("000000"   black .. "ffffff"   white)
        RGBA:      rrggbbaa    (rgb with alpha)
    
Example:
    "PX 420 69 ff\n"       -> set the color of pixel at (420, 69) to white
    "PX 420 69 00ffff\n"   -> set the color of pixel at (420, 69) to cyan
    "PX 420 69 ffff007f\n" -> blend the color of pixel at (420, 69) with yellow (alpha 127)
"#;

macro_rules! print_and_return_error {
    ($($arg:tt)+) => {
        {
            error!($($arg)+);
            return Err(eyre::eyre!($($arg)+));
        }
    }
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

fn setup_logging() -> eyre::Result<()> {
    if cfg!(debug_assertions) {
        let filter = EnvFilter::builder()
            .with_default_directive(Level::DEBUG.into())
            .from_env_lossy();

        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .pretty()
            .with_file(true)
            .with_line_number(true)
            .with_thread_names(true)
            .without_time()
            .finish();
        tracing::subscriber::set_global_default(subscriber)?;
    } else {
        let filter = EnvFilter::builder()
            .with_default_directive(Level::INFO.into())
            .from_env_lossy();

        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .compact()
            .with_thread_names(true)
            .finish();
        tracing::subscriber::set_global_default(subscriber)?;
    }

    Ok(())
}

fn configure_firewall(
    connections_per_ip: Option<NonZeroU32>,
    port: u16,
) -> eyre::Result<Option<Arc<ConnectionLimit>>> {
    match connections_per_ip.map(|connections_per_ip| {
        debug!("enforcing connection limit…");
        Arc::new(ConnectionLimit::new(
            port,
            connections_per_ip.get(),
        ))
    }) {
        None => Ok(None),
        Some(firewall) => {
            match firewall.apply() {
                Ok(()) => Ok(Some(firewall)),
                Err(NftablesError::NftFailed {
                        program,
                        mut stdout,
                        mut stderr,
                        hint,
                    }) => Err(eyre::eyre!("unable to enforce connection limits: {program} returned with an error while {hint}{}{}",
                if !stdout.is_empty() { stdout.insert(0, '\n'); stdout.as_str()} else { "" },
                if !stderr.is_empty() { stderr.insert(0, '\n'); stderr.as_str()} else { "" })),
                Err(e) => Err(eyre::eyre!("unable to enforce connection limits: {e} (Is nftables installed?)")),
            }
        }
    }
}

fn main() -> eyre::Result<()> {
    setup_logging()?;
    let args = cli::Args::parse();
    let firewall = configure_firewall(args.connections_per_ip, args.port)?;

    let mut workers = match core_affinity::get_core_ids() {
        Some(cores) => cores,
        None => print_and_return_error!("unable to get core ids"),
    };
    let main_core = workers.pop().unwrap();
    core_affinity::set_for_current(main_core);

    let cores = NonZeroUsize::new(workers.len()).unwrap();

    let mut workers = workers
        .into_iter()
        .enumerate()
        .take(args.threads.unwrap_or(cores).get())
        .map(|(i, core)| {
            let (rx, tx) = os_pipe::pipe().expect("unable to create pipe");
            let args = args.clone();
            (
                tx,
                thread::spawn(move || {
                    if core_affinity::set_for_current(core) {
                        debug!("[worker: {i}] bound to core {core:?}");
                    } else {
                        warn!("[worker: {i}] unable to bind core {core:?}");
                    }
                    worker(rx, args, i)
                }),
            )
        })
        .collect::<Vec<_>>();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async_main(args, workers.as_mut_slice()))?;

    for (i, (mut tx, join_handle)) in workers.into_iter().enumerate() {
        let msg = Box::new(WorkerMessage::Exit);
        let raw = Box::into_raw(msg);
        tx.write_all(&(raw as u64).to_be_bytes())?;

        match join_handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("worker {i} failed: {e}"),
            Err(_) => error!("unable to join worker thread {i}"),
        }
    }

    drop(firewall);
    Ok(())
}

async fn async_main(
    args: Args,
    workers: &mut [(PipeWriter, thread::JoinHandle<eyre::Result<()>>)],
) -> eyre::Result<()> {
    let clients: Arc<RwLock<Vec<Arc<UserState>>>> = Default::default();

    let socket6 = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    socket6.set_only_v6(true)?;
    socket6.set_reuse_address(true)?;
    socket6.set_nonblocking(true)?;
    socket6.bind(&SockAddr::from(SocketAddr::from((
        Ipv6Addr::UNSPECIFIED,
        args.port,
    ))))?;
    socket6.listen(args.tcp_accept_backlog.get() as c_int)?;

    let socket4 = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket4.set_reuse_address(true)?;
    socket4.set_nonblocking(true)?;
    socket4.bind(&SockAddr::from(SocketAddr::from((
        Ipv4Addr::UNSPECIFIED,
        args.port,
    ))))?;
    socket4.listen(args.tcp_accept_backlog.get() as c_int)?;

    let socket6 = tokio::net::TcpListener::from_std(socket6.into())?;
    let socket4 = tokio::net::TcpListener::from_std(socket4.into())?;

    'tcp_accept_loop: loop {
        for (tx, _) in &mut *workers {
            tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => {
                    info!("shutting down...");
                    break 'tcp_accept_loop;
                },
                res = socket6.accept() => accept_client(clients.clone(), tx, args.ipv4_mask, args.ipv6_mask, args.connection_buffer_size.get(), res?).await?,
                res = socket4.accept() => accept_client(clients.clone(), tx, args.ipv4_mask, args.ipv6_mask, args.connection_buffer_size.get(), res?).await?,
            }
        }
    }

    Ok(())
}

async fn accept_client(
    clients: Arc<RwLock<Vec<Arc<UserState>>>>,
    tx: &mut PipeWriter,
    ipv4_mask: Ipv4Addr,
    ipv6_mask: Ipv6Addr,
    buffer_size: usize,
    (stream, address): (TcpStream, SocketAddr),
) -> eyre::Result<()> {
    debug!("new connection from {address}");
    let socket_fd = stream.into_std()?.into_raw_fd();

    let (uid, state) = {
        let mut clients = clients.write().await;
        get_or_create_user_state(clients.as_mut(), address.ip(), ipv4_mask, ipv6_mask)
    };

    let msg = Box::new(WorkerMessage::NewClient(NewClientMessage {
        socket_fd,
        address,
        uid,
        state,
        buffer_size,
    }));
    let raw = Box::into_raw(msg);

    let mut tx = unsafe { tokio_pipe::PipeWrite::from_raw_fd(tx.as_raw_fd()) };
    tx.write_u64(raw as u64).await?;

    _ = tx.into_raw_fd();
    Ok(())
}
