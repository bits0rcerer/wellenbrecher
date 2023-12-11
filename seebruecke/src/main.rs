use clap::Parser;
use tracing::Level;
use tracing_subscriber::EnvFilter;
use wgpu::Backends;
use winit::event_loop::EventLoop;
use winit::window::WindowBuilder;

use seebruecke::run;
use wellenbrecher_canvas::{Bgra, Canvas};

mod cli;

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

fn main() -> eyre::Result<()> {
    setup_logging()?;

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_decorations(true)
        .with_resizable(true)
        .with_title("Wellenbrecher")
        .build(&event_loop)
        .unwrap();

    let args = cli::Args::parse();
    if args.list_gpus {
        let instance = wgpu::Instance::default();

        let surface = unsafe { instance.create_surface(&window) }.unwrap();
        for (i, a) in instance
            .enumerate_adapters(Backends::all())
            .filter(|a| a.is_surface_supported(&surface))
            .enumerate()
        {
            println!("{i}: {:?}", a.get_info())
        }

        return Ok(());
    }

    let canvas = Canvas::open(
        args.canvas_file_link.as_ref(),
        true,
        args.width.get(),
        args.height.get(),
        Bgra::from_bw(0),
    )?;

    pollster::block_on(run(canvas, event_loop, window, args.gpu_index))
}
