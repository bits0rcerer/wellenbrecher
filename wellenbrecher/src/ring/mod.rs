mod command;
mod command_ring;
pub mod pixelflut_connection_handler;
pub mod ring_coordination;
pub mod write_buffer_drop;

rummelplatz::io_uring! {pixel_flut_ring,
    pixelflut_connection_handler: crate::ring::pixelflut_connection_handler::PixelflutConnectionHandler,
    write_buffer_drop: crate::ring::write_buffer_drop::WriteBufferDrop,
    coordination: crate::ring::ring_coordination::RingCoordination
}
