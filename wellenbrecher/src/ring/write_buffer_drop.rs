use io_uring::squeue::Entry;

use crate::ring::{ControlFlow, RingOperation, SubmissionQueueSubmitter};

#[derive(Debug)]
pub struct WriteBufferDrop;

impl RingOperation for WriteBufferDrop {
    type RingData = Option<Box<[u8]>>;

    fn setup<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        Ok(())
    }

    #[inline]
    fn on_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _: io_uring::cqueue::Entry,
        buf: Self::RingData,
        _: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> ControlFlow {
        drop(buf);
        ControlFlow::Continue
    }

    fn on_teardown_completion<W: Fn(&mut Entry, Self::RingData)>(
        &mut self,
        _: io_uring::cqueue::Entry,
        buf: Self::RingData,
        _: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()> {
        drop(buf);
        Ok(())
    }
}
