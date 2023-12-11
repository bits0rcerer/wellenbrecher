use clap::Parser;
use tracing::Level;
use tracing_subscriber::EnvFilter;

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

    let args = cli::Args::parse();

    let _canvas = Canvas::open(
        args.canvas_file_link.as_ref(),
        true,
        args.width.get(),
        args.height.get(),
        Bgra::from_bw(0),
    )?;

    match &args.command {
        _ => todo!(),
    }
}
