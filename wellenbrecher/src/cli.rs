use std::net::{Ipv4Addr, Ipv6Addr};
use std::num::{NonZeroU32, NonZeroUsize};

use clap::Parser;

#[derive(Parser, Clone)]
#[command(author, version, about)]
pub struct Args {
    /// Canvas width
    #[arg(long, default_value_t = NonZeroU32::new(1280).unwrap(), env = "CANVAS_WIDTH")]
    pub width: NonZeroU32,

    /// Canvas height
    #[arg(long, default_value_t = NonZeroU32::new(720).unwrap(), env = "CANVAS_HEIGHT")]
    pub height: NonZeroU32,

    /// Limit the number of OS threads
    #[arg(short = 'n', long, env = "WELLENBRECHER_THREAD_LIMIT")]
    pub threads: Option<NonZeroUsize>,

    #[arg(
        short,
        long,
        help = r#"Max connections per ip
    
This option requires permissions to alter nftables on your system. You need to run with 'CAP_NET_ADMIN' or as root.

You can get an elevated shell with:
    $ sudo --preserve-env=USER \
        capsh --caps="cap_net_admin+eip cap_setpcap,cap_setuid,cap_setgid+ep" --keep=1 --addamb=cap_net_admin \
        --user="$USER" -- -c "$SHELL"
"#,
        env = "WELLENBRECHER_CONNECTIONS_PER_IP"
    )]
    pub connections_per_ip: Option<NonZeroU32>,

    /// Port pixelflut will run on
    #[arg(short, long, default_value_t = 1337, env = "PORT")]
    pub port: u16,

    /// IPv4 mask for the bits identifying a player
    #[arg(long, default_value_t = Ipv4Addr::from([0xff, 0xff, 0xff, 0xff]), env = "WELLENBRECHER_IPV4_MASK")]
    pub ipv4_mask: Ipv4Addr,

    /// IPv6 mask for the bits identifying a player
    #[arg(long, default_value_t = Ipv6Addr::from([0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff]), env = "WELLENBRECHER_IPV6_MASK")]
    pub ipv6_mask: Ipv6Addr,

    /// buffer size per connection in bytes
    #[arg(long = "buffer", default_value_t = unsafe { NonZeroUsize::new_unchecked(64 * 1024) }, env = "WELLENBRECHER_BUFFER_PER_CONNECTION")]
    pub connection_buffer_size: NonZeroUsize,

    /// io_uring ring size for the empress and lackey rings
    #[arg(long, default_value_t = unsafe { NonZeroU32::new_unchecked(1024) }, env = "WELLENBRECHER_IO_URING_SIZE")]
    pub io_uring_size: NonZeroU32,

    /// TCP Socket backlog
    #[arg(long, default_value_t = unsafe { NonZeroU32::new_unchecked(128) }, env = "WELLENBRECHER_TCP_BACKLOG")]
    pub tcp_accept_backlog: NonZeroU32,

    /// Canvas shared memory file link
    #[arg(short = 'l', long, default_value_t = String::from("/tmp/wellenbrecher-canvas"), env = "WELLENBRECHER_CANVAS_FLINK")]
    pub canvas_file_link: String,

    /// Removes the shared canvas and exits immediately
    #[arg(long, default_value_t = false)]
    pub remove_canvas: bool,

    /// Hide the banner
    #[arg(long, default_value_t = false, env = "WELLENBRECHER_HIDE_BANNER")]
    pub no_banner: bool,
}
