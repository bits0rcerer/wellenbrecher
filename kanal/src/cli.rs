use std::num::{NonZeroU16, NonZeroU32};

use clap::{Parser, Subcommand};

#[derive(Parser, Clone, Debug)]
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

    /// Canvas shared memory file link
    #[arg(short, long, default_value_t = NonZeroU16::new(30).unwrap())]
    pub fps: NonZeroU16,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Clone, Debug)]
pub enum Commands {}
