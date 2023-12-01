use std::num::NonZeroU32;
use std::sync::Arc;

use clap::Parser;
use nftables::helper::NftablesError;
use tracing::{debug, Level};
use tracing_subscriber::EnvFilter;

use crate::firewall::ConnectionLimit;

mod canvas;
mod cli;
mod firewall;

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

    drop(firewall);
    Ok(())
}
