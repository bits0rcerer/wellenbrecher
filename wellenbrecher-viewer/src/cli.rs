use std::num::NonZeroU32;

use clap::Parser;

#[derive(Parser, Clone)]
#[command(author, version, about)]
pub struct Args {
    /// Canvas width
    #[arg(long, default_value_t = NonZeroU32::new(1280).unwrap())]
    pub width: NonZeroU32,

    /// Canvas height
    #[arg(long, default_value_t = NonZeroU32::new(720).unwrap())]
    pub height: NonZeroU32,

    /// GPU Index
    #[arg(long, default_value_t = 0usize)]
    pub gpu_index: usize,

    /// List available GPUs
    #[arg(long = "list-gpus", default_value_t = false)]
    pub list_gpus: bool,
}
