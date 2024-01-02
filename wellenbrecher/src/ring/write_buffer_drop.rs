use rummelplatz::io_uring::squeue::Entry;
use rummelplatz::{ControlFlow, RingOperation, SubmissionQueueSubmitter};

#[derive(Debug)]
pub struct WriteBufferDrop;

#[derive(Debug)]
pub enum WriteBufferDropDescriptor {
    None,
    Buffer(Box<[u8]>),
    IoVec(Vec<libc::iovec>),
}

impl RingOperation for WriteBufferDrop {
    type RingData = WriteBufferDropDescriptor;
    type SetupError = eyre::Error;
    type TeardownError = eyre::Error;
    type ControlFlowWarn = eyre::Error;
    type ControlFlowError = eyre::Error;

    fn setup<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        Ok(())
    }

    #[inline]
    fn on_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _: rummelplatz::io_uring::cqueue::Entry,
        buf: Self::RingData,
        _: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> (
        ControlFlow<Self::ControlFlowWarn, Self::ControlFlowError>,
        Option<Self::RingData>,
    ) {
        drop(buf);
        (ControlFlow::Continue, None)
    }

    fn on_teardown_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _: rummelplatz::io_uring::cqueue::Entry,
        buf: Self::RingData,
        _: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        drop(buf);
        Ok(())
    }
}
