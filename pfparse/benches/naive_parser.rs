use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rand::prelude::SliceRandom;
use rand::thread_rng;

use pfparse::{Command, CommandExecutionError, CommandHandler, NaiveParser, PixelflutParser};

fn command_data(line_break: &str, frames: usize) -> Box<[u8]> {
    let mut commands = vec![];
    for _ in 0..frames {
        for y in 0..720 {
            for x in 0..1280 {
                let r = x as u8 % u8::MAX;
                let g = y as u8 % u8::MAX;
                let b = (x + y) as u8 % u8::MAX;

                commands.push(format!("PX {x} {y} {r:02x}{g:02x}{b:02x}{line_break}"))
            }
        }
    }
    commands.shuffle(&mut thread_rng());
    commands.insert(0, format!("OFFSET 0 0{line_break}"));
    commands.insert(0, format!("SIZE{line_break}"));
    commands.insert(0, format!("HELP{line_break}"));

    commands
        .into_iter()
        .flat_map(|cmd| cmd.into_bytes())
        .collect()
}

fn criterion_benchmark(c: &mut Criterion) {
    #[derive(Debug, Eq, PartialEq)]
    struct Infallible;
    impl CommandExecutionError for Infallible {}
    struct NopHandler(u64);
    impl CommandHandler for NopHandler {
        type Error = Infallible;

        #[inline]
        fn handle(&mut self, cmd: Command) -> Result<(), Self::Error> {
            std::hint::black_box(cmd);
            self.0 = self.0.wrapping_add(1);
            Ok(())
        }
    }

    let test_command_set = command_data("\n", 1);
    c.bench_function(
        format!(
            "NaiveParser.feed({} mBytes)",
            test_command_set.len() as f32 / 1024f32 / 1024f32
        )
        .as_str(),
        |b| {
            b.iter(|| {
                NaiveParser
                    .feed(
                        black_box(test_command_set.as_ref()),
                        black_box(&mut NopHandler(0)),
                    )
                    .expect("feed failed")
            })
        },
    );
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
