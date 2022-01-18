# Aleo Light Prover

## Introduction

A standalone Aleo prover build upon snarkOS and snarkVM, with multi-threading optimization.

It's called "light" because it won't spin up a full node, but instead will only run the prover part.

This prover only supports operators using [my modified code](https://github.com/HarukaMa/snarkOS) as it relies on the custom messaging protocol to work properly.

## Building

Install the dependencies:

```
rust (>= 1.56)
clang
libssl-dev
pkg-config
```

Run `cargo build --release` to build the binary.

## Usage

Please refer to the usage help (`target/release/aleo-prover --help`):

```
prover 0.1.0
Standalone prover.

USAGE:
    aleo-prover [FLAGS] [OPTIONS] --address <address> --pool <pool>

FLAGS:
    -d, --debug      Enable debug logging
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -a, --address <address>    Prover address (aleo1...)
    -p, --pool <pool>          Pool address:port
    -t, --threads <threads>    Number of threads
```

Use your own address as the prover address, not the pool's address.

## Optimizations

You can specify the number of threads to use by `--threads` option. The prover will use all available threads by default, but it won't use 100% of them all the time. You can try adjusting the number to see if using more threads gives better proof rate.

You can enable debug logging by `--debug` option. When starting the prover, there will be a debug log which outputs the structure of the thread pools.

## GPU support?

Still working on it.

## License

GPL-3.0-or-later