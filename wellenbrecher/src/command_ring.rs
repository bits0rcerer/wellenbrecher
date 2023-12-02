use std::slice::from_raw_parts;

use paste::paste;
use thiserror::Error;

use wellenbrecher_canvas::Bgra;

use crate::command::Command;

#[derive(Debug)]
pub struct CommandRing {
    ptr: *mut u8,
    end: *mut u8,
    len: usize,

    read: *mut u8,
    write: *mut u8,
    last_op: Operation,
}

#[derive(Debug)]
enum Operation {
    Read,
    Write,
}

const HELP_VERB: &str = "HELP\n";
const SIZE_VERB: &str = "SIZE\n";
const PX_VERB: &str = "PX";

impl Drop for CommandRing {
    fn drop(&mut self) {
        let vector = unsafe { Vec::from_raw_parts(self.ptr, 0, self.len) };
        drop(vector);
    }
}

macro_rules! impl_consume_decimal_u32_until {
    ($name:ident, $($character:literal),+) => {
        paste! {
            impl CommandRing {
                #[inline]
                fn [<consume_decimal_u32_until_ $name>](&mut self) -> RingResult<(u32, u8)> {
                    if self.available_to_read() == 0 {
                        return Err(CommandRingError::MoreDataRequired);
                    }

                    let mut value = 0u32;

                    // unrolled for first iteration
                    unsafe {
                        {
                            if self.available_to_read() == 0 {
                                return Err(CommandRingError::MoreDataRequired);
                            }
                            let digit = self.read.read();
                            self.increment_read_unchecked();

                            let digit = digit.wrapping_sub(b'0');
                            if digit >= 10 {
                                return Err(CommandRingError::InvalidDecimalDigit(digit.wrapping_add(b'0') as char));
                            }

                            value = (value * 10) + digit as u32;
                        }

                        loop {
                            if self.read == self.write {
                                return Err(CommandRingError::MoreDataRequired);
                            }
                            let digit = self.read.read();
                            self.increment_read_unchecked();

                            $(
                            if digit == $character {
                                return Ok((value, digit));
                            }
                            )+

                            let digit = digit.wrapping_sub(b'0');
                            if digit >= 10 {
                                return Err(CommandRingError::InvalidDecimalDigit(digit.wrapping_add(b'0') as char));
                            }

                            value = (value * 10) + digit as u32;
                        }
                    }
                }
            }
        }
    };
}

impl_consume_decimal_u32_until!(whitespace, b' ');
impl_consume_decimal_u32_until!(whitespace_or_new_line, b' ', b'\n');

macro_rules! impl_advance {
    ($(($pointer:tt, $op:expr)),+) => {
        paste! {
            #[allow(dead_code)]
            impl CommandRing {
                $(
                #[inline]
                pub unsafe fn [<advance_ $pointer _unchecked>](&mut self, offset: usize) {
                    let offset = (self.$pointer.offset_from(self.ptr) as usize + offset) % self.len;
                    self.$pointer = self.ptr.add(offset);
                    self.last_op = $op;
                }

                #[inline]
                pub unsafe fn [<increment_ $pointer _unchecked>](&mut self) {
                    self.$pointer = self.$pointer.add(1);
                    if self.$pointer == self.end {
                        self.$pointer = self.ptr;
                    }
                    self.last_op = $op;
                }
                )+
            }
        }
    };
}

impl_advance!((read, Operation::Read), (write, Operation::Write));

impl CommandRing {
    pub fn new(size: usize) -> Self {
        let (ptr, _, len) = Vec::with_capacity(size).into_raw_parts();

        unsafe {
            Self {
                ptr,
                end: ptr.add(len),
                len,
                read: ptr,
                write: ptr,
                last_op: Operation::Read,
            }
        }
    }

    #[inline]
    pub fn contig_write(&self) -> (*mut u8, u32) {
        unsafe {
            let len = self.read.offset_from(self.write);

            match len {
                n if n < 0 => (self.write, self.end.offset_from(self.write) as u32),
                0 => match self.last_op {
                    Operation::Read => (self.write, self.end.offset_from(self.write) as u32),
                    Operation::Write => (self.write, 0),
                },
                len => (self.write, len as u32),
            }
        }
    }

    #[inline]
    fn contig_read(&self) -> u32 {
        unsafe {
            let len = self.write.offset_from(self.read);

            match len {
                n if n < 0 => self.end.offset_from(self.read) as u32,
                0 => match self.last_op {
                    Operation::Read => 0,
                    Operation::Write => self.end.offset_from(self.read) as u32,
                },
                len => len as u32,
            }
        }
    }

    #[inline]
    fn available_to_read(&self) -> usize {
        unsafe {
            let n = self.write.offset_from(self.read);

            match n {
                n if n < 0 => (self.len as isize + n) as usize,
                0 => match self.last_op {
                    Operation::Read => 0,
                    Operation::Write => self.len,
                },
                n => n as usize,
            }
        }
    }

    #[inline]
    pub fn read_next_command(&mut self) -> RingResult<Command> {
        let old_read = self.read;

        match self.read_next_command_inner() {
            Ok(cmd) => Ok(cmd),
            Err(CommandRingError::MoreDataRequired) => {
                self.read = old_read;
                Err(CommandRingError::MoreDataRequired)
            }
            Err(e) => Err(e),
        }
    }

    #[inline]
    fn consume_compare(&mut self, other: &str) -> RingResult<bool> {
        let contig_read = self.contig_read() as usize;
        let other_len = other.len();

        if contig_read >= other_len {
            #[allow(clippy::transmute_bytes_to_str)]
            let in_ring: &str =
                unsafe { std::mem::transmute(from_raw_parts(self.read, other_len)) };
            return if in_ring == other {
                unsafe { self.advance_read_unchecked(other_len) };
                Ok(true)
            } else {
                Ok(false)
            };
        }

        if self.available_to_read() >= other_len {
            #[allow(clippy::transmute_bytes_to_str)]
            let in_ring_a: &str = unsafe {
                std::mem::transmute(from_raw_parts(
                    self.read,
                    self.end.offset_from(self.read) as usize,
                ))
            };

            if in_ring_a != &other[0..in_ring_a.len()] {
                return Ok(false);
            }

            let in_ring_b: &str = unsafe {
                #[allow(clippy::transmute_bytes_to_str)]
                std::mem::transmute(from_raw_parts(
                    self.ptr,
                    other_len - self.end.offset_from(self.read) as usize,
                ))
            };

            if in_ring_b != &other[in_ring_a.len()..] {
                return Ok(false);
            }

            unsafe {
                self.advance_read_unchecked(other_len);
            }
            return Ok(true);
        }

        Err(CommandRingError::MoreDataRequired)
    }

    #[inline]
    fn consume_whitespace(&mut self) -> RingResult<()> {
        if self.available_to_read() == 0 {
            return Err(CommandRingError::MoreDataRequired);
        }

        unsafe {
            while self.read.read() == b' ' {
                self.increment_read_unchecked();

                if self.read == self.write {
                    return Err(CommandRingError::MoreDataRequired);
                }
            }
        }

        Ok(())
    }

    #[inline]
    fn consume_hexadecimal_color_until_new_line(&mut self) -> RingResult<Bgra> {
        if self.available_to_read() == 0 {
            return Err(CommandRingError::MoreDataRequired);
        }

        let mut value = 0u32;

        // unrolled for first and second iteration
        unsafe {
            {
                let chr = self.read.read();
                self.increment_read_unchecked();

                let digit = if chr >= b'a' {
                    chr.wrapping_sub(b'a' - 10)
                } else {
                    chr.wrapping_sub(b'0')
                };

                if digit >= 16 {
                    return Err(CommandRingError::InvalidHexadecimalDigit(chr as char));
                }

                value = (value << 4) + digit as u32;
            }
            {
                if self.read == self.write {
                    return Err(CommandRingError::MoreDataRequired);
                }
                let chr = self.read.read();
                self.increment_read_unchecked();

                let digit = if chr >= b'a' {
                    chr.wrapping_sub(b'a' - 10)
                } else {
                    chr.wrapping_sub(b'0')
                };

                if digit >= 16 {
                    return Err(CommandRingError::InvalidHexadecimalDigit(chr as char));
                }

                value = (value << 4) + digit as u32;
            }

            for len in 2.. {
                if self.read == self.write {
                    return Err(CommandRingError::MoreDataRequired);
                }
                let chr = self.read.read();
                self.increment_read_unchecked();

                if chr == b'\n' {
                    return match len {
                        6 => Ok(Bgra::from_rgb(value)),
                        2 => Ok(Bgra::from_bw(value as u8)),
                        8 => Ok(Bgra::from_rgba(value)),
                        _ => Err(CommandRingError::InvalidColor),
                    };
                }

                let digit = if chr >= b'a' {
                    chr.wrapping_sub(b'a' - 10)
                } else {
                    chr.wrapping_sub(b'0')
                };

                if digit >= 16 {
                    return Err(CommandRingError::InvalidHexadecimalDigit(chr as char));
                }

                value = (value << 4) + digit as u32;
            }

            unreachable!()
        }
    }

    #[inline]
    fn read_next_command_inner(&mut self) -> RingResult<Command> {
        if self.consume_compare(PX_VERB)? {
            self.consume_whitespace()?;
            let (x, _) = self.consume_decimal_u32_until_whitespace()?;
            self.consume_whitespace()?;
            let (y, terminator) = self.consume_decimal_u32_until_whitespace_or_new_line()?;

            if terminator == b' ' {
                let color = self.consume_hexadecimal_color_until_new_line()?;
                Ok(Command::SetPixel { x, y, color })
            } else {
                Ok(Command::GetPixel { x, y })
            }
        } else if self.consume_compare(SIZE_VERB)? {
            Ok(Command::Size)
        } else if self.consume_compare(HELP_VERB)? {
            Ok(Command::Help)
        } else {
            Err(CommandRingError::UnknownVerb)
        }
    }
}

type RingResult<T> = Result<T, CommandRingError>;

#[derive(Error, Debug, Copy, Clone)]
pub enum CommandRingError {
    #[error("the operation requires more data in this buffer")]
    MoreDataRequired,
    #[error("got an invalid decimal digit \"{0}\"")]
    InvalidDecimalDigit(char),
    #[error("got an invalid hexadecimal digit \"{0}\"")]
    InvalidHexadecimalDigit(char),
    #[error("got an invalid color")]
    InvalidColor,
    #[error("got an unknown verb")]
    UnknownVerb,
}
