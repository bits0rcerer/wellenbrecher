#![feature(test)]
extern crate test;

use std::num::ParseIntError;
use std::str::{FromStr, Utf8Error};

use thiserror::Error;

use wellenbrecher_canvas::Bgra;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Command {
    Help,
    Size,
    Offset { x: u16, y: u16 },
    GetPixel { x: u16, y: u16 },
    SetPixel { x: u16, y: u16, color: Bgra },
}

pub trait CommandHandler {
    type Error;

    fn handle(&mut self, cmd: Command) -> Result<(), Self::Error>;
}

pub trait PixelflutParser {
    fn feed<E: CommandExecutionError>(
        &mut self,
        data: &[u8],
        handler: &mut impl CommandHandler<Error = E>,
    ) -> Result<(), ParserError<E>>;
}

pub struct NaiveParser;

impl PixelflutParser for NaiveParser {
    #[inline]
    fn feed<E: CommandExecutionError>(
        &mut self,
        data: &[u8],
        handler: &mut impl CommandHandler<Error = E>,
    ) -> Result<(), ParserError<E>> {
        for cmd in data.split(|b| *b == b'\n') {
            match cmd {
                [] => {}
                [b'H', b'E', b'L', b'P'] | [b'H', b'E', b'L', b'P', b'\r'] => {
                    handler.handle(Command::Help)?;
                }
                [b'S', b'I', b'Z', b'E'] | [b'S', b'I', b'Z', b'E', b'\r'] => {
                    handler.handle(Command::Size)?;
                }
                [b'O', b'F', b'F', b'S', b'E', b'T', b' ', cords @ .., b'\r']
                | [b'O', b'F', b'F', b'S', b'E', b'T', b' ', cords @ ..] => {
                    match cords
                        .split(|b| *b == b' ')
                        .collect::<Vec<&[u8]>>()
                        .as_slice()
                    {
                        [x, y] => {
                            handler.handle(Command::Offset {
                                x: u16::from_str(std::str::from_utf8(x)?)?,
                                y: u16::from_str(std::str::from_utf8(y)?)?,
                            })?;
                        }
                        _ => return Err(ParserError::InvalidCoordinates),
                    }
                }
                [b'P', b'X', b' ', params @ .., b'\r'] | [b'P', b'X', b' ', params @ ..] => {
                    match params
                        .split(|b| *b == b' ')
                        .collect::<Vec<&[u8]>>()
                        .as_slice()
                    {
                        [x, y, color] => {
                            let color = match std::str::from_utf8(color)? {
                                argb if argb.len() == 6 => {
                                    Bgra::from_rgb(u32::from_str_radix(argb, 16)?)
                                }
                                rgb if rgb.len() == 8 => {
                                    Bgra::from_argb(u32::from_str_radix(rgb, 16)?)
                                }
                                bw if bw.len() == 2 => Bgra::from_bw(u8::from_str_radix(bw, 16)?),
                                _ => return Err(ParserError::InvalidColor),
                            };

                            handler.handle(Command::SetPixel {
                                x: u16::from_str(std::str::from_utf8(x)?)?,
                                y: u16::from_str(std::str::from_utf8(y)?)?,
                                color,
                            })?;
                        }
                        [x, y] => {
                            handler.handle(Command::GetPixel {
                                x: u16::from_str(std::str::from_utf8(x)?)?,
                                y: u16::from_str(std::str::from_utf8(y)?)?,
                            })?;
                        }
                        _ => return Err(ParserError::InvalidCoordinates),
                    }
                }
                _ => return Err(ParserError::UnknownCommand),
            }
        }

        Ok(())
    }
}

pub trait CommandExecutionError: PartialEq {}

#[derive(Debug, Error, PartialEq)]
pub enum ParserError<E: CommandExecutionError> {
    #[error("unknown command")]
    UnknownCommand,
    #[error("invalid coordinates")]
    InvalidCoordinates,
    #[error("invalid color")]
    InvalidColor,
    #[error("invalid character: {}", 0)]
    InvalidCharacter(#[from] Utf8Error),
    #[error("invalid integer: {}", 0)]
    InvalidInteger(#[from] ParseIntError),
    #[error("command execution error: {}", 0)]
    CommandExecutionError(#[from] E),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naive_parser_test() {
        #[derive(PartialEq, Debug)]
        struct Infallible;
        impl CommandExecutionError for Infallible {}

        struct Handler {
            latest: Command,
        }
        impl CommandHandler for Handler {
            type Error = Infallible;

            fn handle(&mut self, cmd: Command) -> Result<(), Self::Error> {
                self.latest = cmd;
                Ok(())
            }
        }

        let mut handler = Handler {
            latest: Command::Size,
        };
        let mut parser = NaiveParser;

        assert_eq!(Ok(()), parser.feed(b"HELP\n", &mut handler));
        assert_eq!(Command::Help, handler.latest);

        assert_eq!(Ok(()), parser.feed(b"HELP\r\n", &mut handler));
        assert_eq!(Command::Help, handler.latest);

        assert_eq!(Ok(()), parser.feed(b"SIZE\n", &mut handler));
        assert_eq!(Command::Size, handler.latest);

        assert_eq!(Ok(()), parser.feed(b"SIZE\r\n", &mut handler));
        assert_eq!(Command::Size, handler.latest);

        assert_eq!(Ok(()), parser.feed(b"OFFSET 420 69\n", &mut handler));
        assert_eq!(Command::Offset { x: 420, y: 69 }, handler.latest);

        assert_eq!(Ok(()), parser.feed(b"OFFSET 420 69\r\n", &mut handler));
        assert_eq!(Command::Offset { x: 420, y: 69 }, handler.latest);

        assert_eq!(Ok(()), parser.feed(b"PX 420 69\n", &mut handler));
        assert_eq!(Command::GetPixel { x: 420, y: 69 }, handler.latest);

        assert_eq!(Ok(()), parser.feed(b"PX 420 69\r\n", &mut handler));
        assert_eq!(Command::GetPixel { x: 420, y: 69 }, handler.latest);

        assert_eq!(Ok(()), parser.feed(b"PX 420 69 ff\n", &mut handler));
        assert_eq!(
            Command::SetPixel {
                x: 420,
                y: 69,
                color: Bgra::from_bw(0xff),
            },
            handler.latest
        );

        assert_eq!(Ok(()), parser.feed(b"PX 420 69 ff\r\n", &mut handler));
        assert_eq!(
            Command::SetPixel {
                x: 420,
                y: 69,
                color: Bgra::from_bw(0xff),
            },
            handler.latest
        );

        assert_eq!(Ok(()), parser.feed(b"PX 420 69 1144ee\n", &mut handler));
        assert_eq!(
            Command::SetPixel {
                x: 420,
                y: 69,
                color: Bgra {
                    r: 0x11,
                    g: 0x44,
                    b: 0xee,
                    a: 0xff,
                },
            },
            handler.latest
        );

        assert_eq!(Ok(()), parser.feed(b"PX 420 69 1144ee\r\n", &mut handler));
        assert_eq!(
            Command::SetPixel {
                x: 420,
                y: 69,
                color: Bgra {
                    r: 0x11,
                    g: 0x44,
                    b: 0xee,
                    a: 0xff,
                },
            },
            handler.latest
        );

        assert_eq!(Ok(()), parser.feed(b"PX 420 69 cc1144ee\n", &mut handler));
        assert_eq!(
            Command::SetPixel {
                x: 420,
                y: 69,
                color: Bgra {
                    r: 0x11,
                    g: 0x44,
                    b: 0xee,
                    a: 0xcc,
                },
            },
            handler.latest
        );

        assert_eq!(Ok(()), parser.feed(b"PX 420 69 cc1144ee\r\n", &mut handler));
        assert_eq!(
            Command::SetPixel {
                x: 420,
                y: 69,
                color: Bgra {
                    r: 0x11,
                    g: 0x44,
                    b: 0xee,
                    a: 0xcc,
                },
            },
            handler.latest
        );
    }
}
