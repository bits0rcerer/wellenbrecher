use std::net::{Ipv4Addr, Ipv6Addr};
use std::num::{NonZeroU32, NonZeroUsize};

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

    /// Limit the number of OS threads
    #[arg(short = 'n', long)]
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
"#
    )]
    pub connections_per_ip: Option<NonZeroU32>,

    /// Hide private IPv4 addresses
    #[arg(long)]
    pub hide_private_ipv4: bool,

    /// Port pixelflut will run on
    #[arg(short, long, default_value_t = 1337)]
    pub port: u16,

    /// IPv4 mask for the bits identifying a player
    #[arg(long, default_value_t = Ipv4Addr::from([0xff, 0xff, 0xff, 0xff]))]
    pub ipv4_mask: Ipv4Addr,

    /// IPv6 mask for the bits identifying a player
    #[arg(long, default_value_t = Ipv6Addr::from([0xffff, 0xffff, 0xffff, 0xffff, 0, 0, 0, 0]))]
    pub ipv6_mask: Ipv6Addr,

    /// buffer size per connection in bytes
    #[arg(long = "buffer", default_value_t = unsafe { NonZeroUsize::new_unchecked(64 * 1024) })]
    pub connection_buffer_size: NonZeroUsize,

    /// io_uring ring size for the and worker rings
    #[arg(long, default_value_t = unsafe { NonZeroU32::new_unchecked(1024) })]
    pub io_uring_size: NonZeroU32,

    /// TCP Socket backlog
    #[arg(long, default_value_t = unsafe { NonZeroU32::new_unchecked(128) })]
    pub tcp_accept_backlog: NonZeroU32,
}
