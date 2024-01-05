#![feature(vec_into_raw_parts)]
#![feature(new_uninit)]
#![feature(const_for)]
#![feature(const_trait_impl)]
#![feature(const_mut_refs)]
#![feature(effects)]

use std::fmt::Debug;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::os::fd::{AsRawFd, RawFd};
use std::os::raw::c_int;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use clap::Parser;
use core_affinity::CoreId;
use nftables::helper::NftablesError;
use shared_memory::ShmemError;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tracing::{debug, error, info, warn, Level};
use tracing_subscriber::EnvFilter;

use wellenbrecher_canvas::{Bgra, Canvas, CanvasCreateInfo};

use crate::cli::Args;
use crate::firewall::ConnectionLimit;
use crate::ring::pixelflut_connection_handler::PixelflutConnectionHandler;
use crate::ring::ring_coordination::{RingCoordination, UserState};
use crate::ring::write_buffer_drop::WriteBufferDrop;

mod cli;
mod firewall;
mod ring;

const HELP_TEXT: &[u8] = br#"Welcome to Pixelflut!

Commands:
    HELP                -> get this information page
    SIZE                -> get the size of the canvas
    PX <x> <y>          -> get the color of pixel (x, y)
    PX <x> <y> <COLOR>  -> set the color of pixel (x, y)
    OFFSET <x> <y>      -> sets an pixel offset for all following commands

    COLOR:
        Grayscale: ww          ("00"       black .. "ff"       white)
        RGB:       rrggbb      ("000000"   black .. "ffffff"   white)
        RGBA:      rrggbbaa    (rgb with alpha)
    
Example:
    "PX 420 69 ff\n"       -> set the color of pixel at (420, 69) to white
    "PX 420 69 00ffff\n"   -> set the color of pixel at (420, 69) to cyan
    "PX 420 69 ffff007f\n" -> blend the color of pixel at (420, 69) with yellow (alpha 127)
"#;

const BANNER: &str = r"
 __      __          ___    ___                  __                          __                      
/\ \  __/\ \        /\_ \  /\_ \                /\ \                        /\ \                     
\ \ \/\ \ \ \     __\//\ \ \//\ \      __    ___\ \ \____  _ __    __    ___\ \ \___      __   _ __  
 \ \ \ \ \ \ \  /'__`\\ \ \  \ \ \   /'__`\/' _ `\ \ '__`\/\`'__\/'__`\ /'___\ \  _ `\  /'__`\/\`'__\
  \ \ \_/ \_\ \/\  __/ \_\ \_ \_\ \_/\  __//\ \/\ \ \ \L\ \ \ \//\  __//\ \__/\ \ \ \ \/\  __/\ \ \/ 
   \ `\___x___/\ \____\/\____\/\____\ \____\ \_\ \_\ \_,__/\ \_\\ \____\ \____\\ \_\ \_\ \____\\ \_\ 
    '\/__//__/  \/____/\/____/\/____/\/____/\/_/\/_/\/___/  \/_/ \/____/\/____/ \/_/\/_/\/____/ \/_/ 


       A capable Pixelflut server for Linux written in Rust 🦀

       github.com/bits0rcerer/wellenbrecher

";

macro_rules! print_and_return_error {
    ($($arg:tt)+) => {
        {
            error!($($arg)+);
            return Err(eyre::eyre!($($arg)+));
        }
    }
}

fn setup_logging() -> eyre::Result<()> {
    if cfg!(debug_assertions) {
        let filter = EnvFilter::builder()
            .with_default_directive(Level::DEBUG.into())
            .from_env_lossy();

        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .compact()
            //.with_file(true)
            //.with_line_number(true)
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
            .with_target(false)
            .with_thread_names(true)
            .finish();
        tracing::subscriber::set_global_default(subscriber)?;
    }

    Ok(())
}

fn configure_firewall(
    connections_per_ip: Option<NonZeroU32>,
    port: u16,
    ipv4_mask: Ipv4Addr,
    ipv6_mask: Ipv6Addr,
) -> eyre::Result<Option<Arc<ConnectionLimit>>> {
    match connections_per_ip.map(|connections_per_ip| {
        debug!("enforcing connection limit…");
        Arc::new(ConnectionLimit::new(
            port,
            connections_per_ip.get(),
            ipv4_mask,
            ipv6_mask,
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

    unsafe {
        let mut sig_set = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(std::ptr::addr_of_mut!(sig_set));
        libc::sigaddset(std::ptr::addr_of_mut!(sig_set), libc::SIGINT);
        libc::sigaddset(std::ptr::addr_of_mut!(sig_set), libc::SIGQUIT);
        libc::sigaddset(std::ptr::addr_of_mut!(sig_set), libc::SIGTERM);

        if libc::sigprocmask(
            libc::SIG_BLOCK,
            std::ptr::addr_of!(sig_set),
            std::ptr::null_mut(),
        ) == -1
        {
            return Err(eyre::eyre!("unable to setup signal handler"));
        }
    };

    let args = cli::Args::parse();
    if args.remove_canvas {
        return remove_canvas(args.canvas_file_link);
    }

    if !args.no_banner {
        println!("{BANNER}");
    }

    let firewall = configure_firewall(
        args.connections_per_ip,
        args.port,
        args.ipv4_mask,
        args.ipv6_mask,
    )?;

    let clients: Arc<RwLock<Vec<Arc<UserState>>>> = Default::default();

    // protect the process of creating or opening the shared memory
    let canvas_open_lock = Arc::new(Mutex::new(()));

    let cores = match core_affinity::get_core_ids() {
        Some(cores) => cores,
        None => print_and_return_error!("unable to get core ids"),
    };
    let mut workers = Vec::new();

    let (fd_rx, primary_core, primary_index) =
        {
            let (fd_tx, fd_rx) = std::sync::mpsc::channel();
            let mut worker_iter = cores
                .into_iter()
                .enumerate()
                .take_while(|(i, _)| args.threads.is_none() || *i < args.threads.unwrap().get());

            let (primary_index, primary_core) = worker_iter.next().unwrap();
            for (i, core) in worker_iter {
                let args = args.clone();
                let fd_tx = fd_tx.clone();
                let canvas_open_lock = canvas_open_lock.clone();
                workers.push(thread::Builder::new().name(format!("Lackey-{i}")).spawn(
                    move || lackey(args.io_uring_size, core, i, args, fd_tx, canvas_open_lock),
                )?);
            }

            (fd_rx, primary_core, primary_index)
        };

    {
        let canvas_open_lock = canvas_open_lock.clone();
        thread::Builder::new()
            .name("Empress".to_string())
            .spawn(move || {
                empress(
                    args.io_uring_size,
                    clients,
                    primary_core,
                    primary_index,
                    args,
                    fd_rx,
                    canvas_open_lock,
                )
            })?
            .join()
            .expect("unable to join Empress thread")?;
    }

    for (i, join_handle) in workers.into_iter().enumerate() {
        match join_handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("worker {i} failed: {e}"),
            Err(_) => error!("unable to join worker thread {i}"),
        }
    }

    drop(firewall);

    info!("Exiting...");
    Ok(())
}

fn empress(
    ring_size: NonZeroU32,
    clients: Arc<RwLock<Vec<Arc<UserState>>>>,
    core: CoreId,
    index: usize,
    args: Args,
    fd_rx: std::sync::mpsc::Receiver<RawFd>,
    canvas_open_lock: Arc<Mutex<()>>,
) -> eyre::Result<()> {
    let ring = ring::pixel_flut_ring::Ring::new_raw_ring(ring_size)?;

    let socket6 = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    socket6.set_only_v6(true)?;
    socket6.set_reuse_address(true)?;
    socket6.bind(&SockAddr::from(SocketAddr::from((
        Ipv6Addr::UNSPECIFIED,
        args.port,
    ))))?;
    socket6.listen(args.tcp_accept_backlog.get() as c_int)?;

    let socket4 = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket4.set_reuse_address(true)?;
    socket4.bind(&SockAddr::from(SocketAddr::from((
        Ipv4Addr::UNSPECIFIED,
        args.port,
    ))))?;
    socket4.listen(args.tcp_accept_backlog.get() as c_int)?;

    let ring_fds = fd_rx.iter().chain([ring.as_raw_fd()]).collect::<Vec<_>>();

    let signal_fd = unsafe {
        let mut sig_set = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(std::ptr::addr_of_mut!(sig_set));
        libc::sigaddset(std::ptr::addr_of_mut!(sig_set), libc::SIGINT);
        libc::sigaddset(std::ptr::addr_of_mut!(sig_set), libc::SIGQUIT);
        libc::sigaddset(std::ptr::addr_of_mut!(sig_set), libc::SIGTERM);

        if libc::sigprocmask(
            libc::SIG_BLOCK,
            std::ptr::addr_of!(sig_set),
            std::ptr::null_mut(),
        ) == -1
        {
            return Err(eyre::eyre!("unable to setup signal handler"));
        }

        match libc::signalfd(-1, std::ptr::addr_of!(sig_set), 0) {
            e if e < 0 => {
                return Err(eyre::eyre!(
                    "unable to setup signal handler: {}",
                    std::io::Error::from_raw_os_error(-e)
                ));
            }
            fd => fd.as_raw_fd(),
        }
    };

    worker(
        core,
        index,
        ring,
        RingCoordination::empress(
            vec![socket6, socket4],
            ring_fds,
            signal_fd,
            args.connection_buffer_size,
            clients,
            args.ipv4_mask,
            args.ipv6_mask,
        ),
        args,
        canvas_open_lock,
    )
}

fn lackey(
    ring_size: NonZeroU32,
    core: CoreId,
    index: usize,
    args: Args,
    fd_tx: std::sync::mpsc::Sender<RawFd>,
    canvas_open_lock: Arc<Mutex<()>>,
) -> eyre::Result<()> {
    let ring = ring::pixel_flut_ring::Ring::new_raw_ring(ring_size)?;
    fd_tx.send(ring.as_raw_fd())?;
    drop(fd_tx);

    worker(
        core,
        index,
        ring,
        RingCoordination::lackey(),
        args,
        canvas_open_lock,
    )
}

fn worker(
    core: CoreId,
    index: usize,
    ring: rummelplatz::io_uring::IoUring,
    coordination: RingCoordination,
    args: Args,
    canvas_open_lock: Arc<Mutex<()>>,
) -> eyre::Result<()> {
    if core_affinity::set_for_current(core) {
        debug!("[worker: {index}] bound to core {core:?}");
    } else {
        warn!("[worker: {index}] unable to bind core {core:?}");
    }

    let canvas = {
        let lock = canvas_open_lock
            .lock()
            .expect("unable to lock canvas_open_lock");

        let canvas = match Canvas::open(
            args.canvas_file_link.as_ref(),
            true,
            Some(CanvasCreateInfo {
                width: args.width.get(),
                height: args.height.get(),
                initial_canvas: vec![
                    Bgra::default();
                    (args.width.get() * args.height.get()) as usize
                ]
                .into_boxed_slice(),
            }),
        ) {
            Ok(canvas) => canvas,
            Err(e) => return Err(e.into()),
        };

        drop(lock);
        canvas
    };

    let mut ring = ring::pixel_flut_ring::Ring::new(
        ring,
        None,
        PixelflutConnectionHandler::new(canvas),
        WriteBufferDrop,
        coordination,
    );

    ring.run::<eyre::Error, eyre::Error, eyre::Error>()?;
    Ok(())
}

fn remove_canvas<P: AsRef<Path> + Debug + Clone>(path: P) -> eyre::Result<()> {
    match shared_memory::ShmemConf::new().flink(path.clone()).open() {
        Ok(mut shmem) => {
            shmem.set_owner(true);
            drop(shmem);
            Ok(())
        }
        Err(ShmemError::LinkDoesNotExist) => Ok(()),
        Err(e) => {
            eprintln!("unable to remove shared canvas: {e:?}");
            eprintln!(
                "you can try to remove it manually by executing:\n\trm /dev/shm$(cat {:?}) {:?}",
                path.clone(),
                path.clone()
            );

            Err(e.into())
        }
    }
}
