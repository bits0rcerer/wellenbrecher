# 🌊 Wellenbrecher 🌊

A capable [pixelflut](https://github.com/defnull/pixelflut) server written in Rust 🦀

## But is it fast?

**Yes**, but [#1](https://github.com/bits0rcerer/wellenbrecher/issues/1)

#### Comparison (out-dated/incomplete)

<details>
  <summary>Test machine</summary>

  ```neofetch
                    -`                    bits0rcerer@bench
                   .o+`                   ----------------------
                  `ooo/                   OS: Arch Linux x86_64
                 `+oooo:                  Host: Alder Lake-H PCH (ERYING G660 ITX) E1.0G
                `+oooooo:                 Kernel: 6.6.3-arch1-1
                -+oooooo+:                Pixelflut Canvas: 1280x720
              `/:-:++oooo+:               CPU: 12th Gen Intel i5-12500H (16) @ 4.500GHz
             `/++++/+++++++:              GPU: Intel Alder Lake-P GT2 [Iris Xe Graphics]
            `/++++++++++++++:             Memory: 16GiB
           `/+++ooooooooooooo/`           NIC: Intel Corporation Ethernet Controller XL710 for 40GbE QSFP+
          ./ooosssso++osssssso+`          Kernel parameter: ... mitigations=off ...
         .oossssso-````/ossssss+`
        -osssssso.      :ssssssso.
       :osssssss/        osssso+++.
      /ossssssss/        +ssssooo/-                               
    `/ossssso+/:-        -:/+osssso+-
   `+sso+:-`                 `.-/+oso:
  `++:.                           `-/+/
  .`                                 `/
  ```

</details>

|                                                                          | tsunami¹ - 1 connection | tsunami¹ - 8 connection | tsunami¹ - 16 connection | tsunami¹ - 24 connection | tsunami¹ - 256 connection |
|--------------------------------------------------------------------------|-------------------------|-------------------------|--------------------------|--------------------------|---------------------------|
| wellenbrecher                                                            | **5.8 Gbit/s**          | **22.3 Gbit/s**         | **23.7 Gbit/s**          | **23.7 Gbit/s**          | **23.6 Gbit/s**           |
| [shoreline](https://github.com/TobleMiner/shoreline)                     | 2.8 Gbit/s              | 9.8 Gbit/s              | 12.8 Gbit/s              | 10.2 Gbit/s              | 16.0 Gbit/s               |
| wellenbrecher - single thread                                            | **5.8 Gbit/s**          | 5.5 Gbit/s              | 5.5 Gbit/s               | 5.5 Gbit/s               | 5.4 Gbit/s                |
| [shoreline](https://github.com/TobleMiner/shoreline)     - single thread | 2.8 Gbit/s              | 7.0 Gbit/s              | 9.5 Gbit/s               | 8.8 Gbit/s               | 9.8 Gbit/s                |
| [pixelnuke](https://github.com/defnull/pixelflut#pixelnuke-c-server)     | 1.7 Gbit/s              | 1.7 Gbit/s              | 1.7 Gbit/s               | 1.7 Gbit/s               | 1.6 Gbit/s                |

¹[`tsunami`](https://github.com/bits0rcerer/tsunami) - my pixelflut client

### `wellenbrecher`

The **core**, handling connections and processing commands.

```
Usage: wellenbrecher [OPTIONS]

Options:
      --width <WIDTH>          Canvas width [default: 1280]
      --height <HEIGHT>        Canvas height [default: 720]
  -n, --threads <THREADS>      Limit the number of OS threads
  -p, --port <PORT>            Port pixelflut will run on [default: 1337]
  -c, --connections-per-ip <CONNECTIONS_PER_IP> Max connections per ip
  ...
  -h, --help                    Print help
  ...
```

### `seebruecke`

Frontend to view the canvas.

```
Usage: seebruecke [OPTIONS]

Options:
      --width <WIDTH>          Canvas width [default: 1280]
      --height <HEIGHT>        Canvas height [default: 720]
      --gpu-index <GPU_INDEX>  GPU Index [default: 0]
      --list-gpus              List available GPUs
  -h, --help                   Print help
  -V, --version                Print version
  ```

- `Esc` Exit
- `Up`/`Down` Select a user for highlighting
- `R` reset highlighting
- `Left`/`Right` Adjust highlighting strength

### `gst-wellenbrecher-src`

[GStreamer](https://gstreamer.freedesktop.org/) source to stream the canvas.

```bash
cargo build --package gst-wellenbrecher-src --release
GST_PLUGIN_PATH=$(pwd)/target/release

gst-launch-1.0 wbsrc ! videoconvert ! autovideosink
  ```

## Requirements

- `wellenbrecher`
    - Rust nightly
    - Linux kernel with io_uring

- `seebruecke`
    - see [wgpu supported platforms](https://github.com/gfx-rs/wgpu#supported-platforms)

- `gst-wellenbrecher-src`
  - [GStreamer](https://gstreamer.freedesktop.org/)

The canvas is shared via shared memory

# Special thanks to

> /pixelnuke (C server)
>
> Server written in C, based on libevent2, OpenGL, GLFW and pthreads. It won't get any faster than this.
> Perfect for fast networks and large groups.
>
> ~ <cite>[defnull](https://github.com/defnull), [Mar 2018](https://github.com/defnull/pixelflut/commit/51143d90ed0631293be1d48565874c44515c0dee)</cite>

> Pixelflut - Multiplayer canvas
>
> Pixelflut is a very simple (and inefficient) ASCII based network protocol to draw pixels on a shared screen.
> It works great as a group activity for novice programmers due to it's low entry barrier, or as an interactive and
> very chaotic art installation at hacker events. If you have a beamer or LED wall, a solid network, and a bunch of
> hackers, give it a try. It's fun.
>
> The idea was born at EasterHegg 2014 and evolved into a recurring part of most CCC events since then.
> Pixelflut really developed it's own live and now there are dozens of server and client implementations available.
> Reddit came up with a similar idea in 2017 with /r/place and scaled it to thousands of global users.
>
> - [cccgoe.de/wiki/Pixelflut](https://cccgoe.de/wiki/Pixelflut)
> - [github.com/defnull/pixelflut](https://github.com/defnull/pixelflut)
>
> ~ <cite>[defnull](https://defnull.de/about.html)</cite>

## ❤️ all the creatures fluting pixels with me
