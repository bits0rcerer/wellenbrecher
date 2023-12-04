use std::collections::VecDeque;
use std::fmt::Debug;
use std::iter::zip;
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use io_uring::cqueue::Entry;
use io_uring::squeue::{EntryMarker, PushError};
use io_uring::SubmissionQueue;
use tracing::warn;

mod command;
mod command_ring;
pub mod pixelflut_connection_handler;
pub mod pixelflut_ring_bridge;
pub mod write_buffer_drop;

#[derive(Debug)]
pub enum ControlFlow {
    Continue,
    Exit,
    Warn(eyre::Error),
    Error(eyre::Error),
}

pub trait RingOperation: Debug {
    type RingData;

    fn setup<W: Fn(&mut io_uring::squeue::Entry, Self::RingData)>(
        &mut self,
        submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()>;
    fn on_completion<W: Fn(&mut io_uring::squeue::Entry, Self::RingData)>(
        &mut self,
        completion_entry: Entry,
        ring_data: Self::RingData,
        submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> ControlFlow;
    fn on_teardown_completion<W: Fn(&mut io_uring::squeue::Entry, Self::RingData)>(
        &mut self,
        completion_entry: Entry,
        ring_data: Self::RingData,
        submitter: SubmissionQueueSubmitter<Self::RingData, W>,
    ) -> eyre::Result<()>;
}

pub struct SubmissionQueueSubmitter<
    'a,
    D,
    W: Fn(&mut E, D),
    E: EntryMarker = io_uring::squeue::Entry,
> {
    sq: SubmissionQueue<'a, E>,
    backlog: &'a mut VecDeque<Box<[E]>>,
    backlog_limit: Option<NonZeroUsize>,
    wrapper: W,
    marker: PhantomData<D>,
}

impl<'a, D, W: Fn(&mut E, D), E: EntryMarker> SubmissionQueueSubmitter<'a, D, W, E> {
    #[inline]
    pub fn push(&mut self, entry: E, data: D) -> Result<(), PushError> {
        self.push_multiple([entry], [data])
    }

    #[inline]
    pub unsafe fn push_raw(&mut self, entry: E) -> Result<(), PushError> {
        self.push_multiple_raw([entry])
    }

    #[inline]
    pub fn push_multiple<const N: usize>(
        &mut self,
        mut entries: [E; N],
        data: [D; N],
    ) -> Result<(), PushError> {
        for (entry, data) in zip(entries.iter_mut(), data.into_iter()) {
            (self.wrapper)(entry, data);
        }

        unsafe { self.push_multiple_raw(entries) }
    }

    #[inline]
    pub unsafe fn push_multiple_raw<const N: usize>(
        &mut self,
        entries: [E; N],
    ) -> Result<(), PushError> {
        match self.sq.push_multiple(entries.as_slice()) {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!(
                    "exceeding ring submission queue, using backlog... (may degrade performance)"
                );

                match self.backlog_limit {
                    None => {
                        self.backlog.push_back(entries.into());
                        Ok(())
                    }
                    Some(limit) => {
                        if self.backlog.len() + entries.len() <= limit.get() {
                            self.backlog.push_back(entries.into());
                            Ok(())
                        } else {
                            Err(e)
                        }
                    }
                }
            }
        }
    }
}

#[allow(dead_code)]
impl<'a, D: Clone, W: Fn(&mut E, D), E: EntryMarker> SubmissionQueueSubmitter<'a, D, W, E> {
    #[inline]
    pub fn push_slice(&mut self, mut entries: Box<[E]>, data: &[D]) -> Result<(), PushError> {
        for (entry, data) in zip(entries.iter_mut(), data.iter()) {
            (self.wrapper)(entry, data.clone());
        }

        unsafe { self.push_slice_raw(entries) }
    }
    #[inline]
    pub unsafe fn push_slice_raw(&mut self, entries: Box<[E]>) -> Result<(), PushError> {
        match self.sq.push_multiple(&entries) {
            Ok(()) => Ok(()),
            Err(e) => match self.backlog_limit {
                None => {
                    self.backlog.push_back(entries);
                    Ok(())
                }
                Some(limit) => {
                    if self.backlog.len() + entries.len() <= limit.get() {
                        self.backlog.push_back(entries);
                        Ok(())
                    } else {
                        Err(e)
                    }
                }
            },
        }
    }
}

macro_rules! ring {
    ($ring_name:ident, $($ring_op_name:ident: $ring_op:path),+) => {
        pub mod $ring_name {
            use std::num::{NonZeroU32, NonZeroUsize};
            use std::collections::VecDeque;
            use std::fmt::{Debug, Formatter};
            use std::marker::PhantomData;
            use tracing::{debug, error, warn};
            use crate::ring::{ControlFlow, RingOperation, SubmissionQueueSubmitter};

            // Enforce trait on $ring_op
            const _: () = {
                fn assert_ring_operation<T: RingOperation>() {}
                fn assert_all() {
                    $(assert_ring_operation::<$ring_op>());+
                }
            };

            #[allow(non_camel_case_types)]
            pub enum UserData {
                $($ring_op_name(<$ring_op as RingOperation>::RingData)),+,
                Cancel(u64),
            }

            impl From<UserData> for u64 {
                #[inline]
                fn from(value: UserData) -> u64 {
                    Box::new(value).into()
                }
            }

            impl From<Box<UserData>> for u64 {
                #[inline]
                fn from(value: Box<UserData>) -> u64 {
                    unsafe { std::mem::transmute(value) }
                }
            }

            impl UserData {
                #[inline]
                unsafe fn from_raw(user_data: u64) -> Box<Self> {
                    std::mem::transmute(user_data)
                }
            }

            pub struct Ring {
                ring: io_uring::IoUring,
                backlog: VecDeque<Box<[io_uring::squeue::Entry]>>,
                backlog_limit: Option<NonZeroUsize>,
                $($ring_op_name: $ring_op),+,
            }

            impl Debug for Ring {
                fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                    let operations = ($(&self.$ring_op_name),+);

                    if f.alternate() {
                        write!(f, r"Ring: {{
    backlog_limit: {:#?},
    backlog: {:#?},
    operations: {:#?},
}}", self.backlog_limit, self.backlog, operations)
                    } else {
                        write!(f, r"Ring: {{ backlog_limit: {:?}, backlog: {:?}, operations: {:?} }}", self.backlog_limit, self.backlog, operations)
                    }
                }
            }

            impl Ring {
                #[tracing::instrument]
                pub fn new(ring_size: NonZeroU32, backlog_limit: Option<NonZeroUsize>, $($ring_op_name: $ring_op),+) -> std::io::Result<Self> {
                    let ring = io_uring::IoUring::builder()
                        .setup_single_issuer()
                        .setup_coop_taskrun()
                        .setup_defer_taskrun()
                        .build(ring_size.get())?;

                    Ok(Self {
                        ring,
                        backlog: Default::default(),
                        backlog_limit,
                        $($ring_op_name),+
                    })
                }

                #[inline]
                fn sqe_wrapper(e: &mut io_uring::squeue::Entry, user_data: UserData) {
                    take_mut::take(e, |e| e.user_data(user_data.into()));
                }

                #[tracing::instrument]
                pub fn run(&mut self) -> eyre::Result<()> {
                    let mut result = Ok(());

                    $(self.$ring_op_name.setup(SubmissionQueueSubmitter {
                        sq: self.ring.submission(),
                        backlog: &mut self.backlog,
                        backlog_limit: self.backlog_limit,
                        wrapper: |e, d| Self::sqe_wrapper(e, UserData::$ring_op_name(d)),
                        marker: PhantomData::<<$ring_op as RingOperation>::RingData>,
                    })?;)+

                    unsafe {
                        'ring_loop: loop {
                            self.ring.submission().sync();
                            self.ring.submit_and_wait(1)?;

                            while let Some(entries) = self.backlog.pop_front() {
                                if let Err(_) = self.ring.submission().push_multiple(&entries) {
                                    self.backlog.push_front(entries);
                                    break;
                                }
                            }

                            self.ring.completion().sync();
                            'completion_loop: for cqe in self.ring.completion_shared().by_ref() {
                                if cqe.user_data() == 0 {
                                    // our user data cannot be 0
                                    // but for example msg_ring_fd produces an empty cqe

                                    // ignore
                                    continue;
                                }

                                let user_data = UserData::from_raw(cqe.user_data());
                                let flow = match *user_data {
                                    $(UserData::$ring_op_name(data) => self.$ring_op_name.on_completion(cqe, data, SubmissionQueueSubmitter {
                                        sq: self.ring.submission_shared(),
                                        backlog: &mut self.backlog,
                                        backlog_limit: self.backlog_limit,
                                        wrapper: |e, d| Self::sqe_wrapper(e, UserData::$ring_op_name(d)),
                                        marker: PhantomData::<<$ring_op as RingOperation>::RingData>,
                                    })),+,
                                    UserData::Cancel(_) => unreachable!(),
                                };

                                match flow {
                                    ControlFlow::Exit => break 'ring_loop,
                                    ControlFlow::Error(e) => {
                                        error!("unable to handle ring completion entry: {e}");
                                        result = Err(e);
                                        break 'ring_loop;
                                    }
                                    ControlFlow::Warn(e) => {
                                        warn!("unable to handle ring completion entry: {e}");
                                        continue 'completion_loop;
                                    }
                                    ControlFlow::Continue => {}
                                }
                            }
                        }
                    }

                    debug!("shutting down ring...");
                    unsafe {
                        // u64::MAX inside the UserData::Cancel should prevent a raw user data to be 0

                        let cancel = io_uring::opcode::AsyncCancel2::new(io_uring::types::CancelBuilder::any())
                            .build()
                            .user_data(UserData::Cancel(u64::MAX).into());
                        self.ring.submission().push(&cancel)?;
                    }

                    unsafe {
                        'cancel_loop: loop {
                            self.ring.submission().sync();
                            self.ring.submit_and_wait(1)?;

                            self.ring.completion().sync();
                            for cqe in self.ring.completion_shared().by_ref() {
                                if cqe.user_data() == 0 {
                                    // our user data cannot be 0
                                    // but for example msg_ring_fd produces an empty cqe

                                    // ignore
                                    continue;
                                }

                                let user_data = UserData::from_raw(cqe.user_data());
                                let teardown_result = match *user_data {
                                    $(UserData::$ring_op_name(data) => self.$ring_op_name.on_teardown_completion(cqe, data, SubmissionQueueSubmitter {
                                        sq: self.ring.submission_shared(),
                                        backlog: &mut self.backlog,
                                        backlog_limit: self.backlog_limit,
                                        wrapper: |e, d| Self::sqe_wrapper(e, UserData::$ring_op_name(d)),
                                        marker: PhantomData::<<$ring_op as RingOperation>::RingData>,
                                    })),+,
                                    UserData::Cancel(u64::MAX) => break 'cancel_loop,
                                    UserData::Cancel(_) => unreachable!(),
                                };

                                if let Err(e) = teardown_result {
                                    error!("unable to handle ring completion entry on teardown: {e}");
                                    result = Err(e);
                                }
                            }
                        }
                    }

                    debug!("ring finished: {result:?}");
                    result
                }
            }
        }
    }
}

ring! {pixel_flut_ring,
    bridge: crate::ring::pixelflut_ring_bridge::Receiver,
    pixelflut_connection_handler: crate::ring::pixelflut_connection_handler::PixelflutConnectionHandler,
    write_buffer_drop: crate::ring::write_buffer_drop::WriteBufferDrop
}
