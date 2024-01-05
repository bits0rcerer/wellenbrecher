use clap::Parser;

#[derive(Parser, Clone)]
#[command(author, version, about)]
pub struct Args {
    /// GPU Index
    #[arg(long, default_value_t = 0usize, env = "SEEBRUECKE_GPU")]
    pub gpu_index: usize,

    /// List available GPUs
    #[arg(long = "list-gpus", default_value_t = false)]
    pub list_gpus: bool,

    /// Start in fullscreen mode
    #[arg(short, long, default_value_t = false, env = "SEEBRUECKE_FULLSCREEN")]
    pub fullscreen: bool,

    /// Canvas shared memory file link
    #[arg(short = 'l', long, default_value_t = String::from("/tmp/wellenbrecher-canvas"), env = "WELLENBRECHER_CANVAS_FLINK")]
    pub canvas_file_link: String,
}
