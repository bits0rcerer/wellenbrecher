use std::num::NonZeroU32;

use clap::{Parser, Subcommand};

#[derive(Parser, Clone)]
#[command(author, version, about)]
pub struct Args {
    /// Canvas width
    #[arg(long, default_value_t = NonZeroU32::new(1280).unwrap())]
    pub width: NonZeroU32,

    /// Canvas height
    #[arg(long, default_value_t = NonZeroU32::new(720).unwrap())]
    pub height: NonZeroU32,

    /// Canvas shared memory file link
    #[arg(short = 'l', long = "canvas-file-link", default_value_t = String::from("/tmp/wellenbrecher-canvas"))]
    pub canvas_file_link: String,
}

#[derive(Subcommand)]
enum Commands {}
