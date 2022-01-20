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
prover 0.2.0
Standalone prover.

USAGE:
    aleo-prover [FLAGS] [OPTIONS] --address <address> --pool <pool>

FLAGS:
    -d, --debug      Enable debug logging
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -a, --address <address>    Prover address (aleo1...)
    -g, --cuda <cuda>...       Indexes of GPUs to use (starts from 0)
                               Specify multiple times to use multiple GPUs
                               Example: -g 0 -g 1 -g 2
                               Note: Pure CPU proving will be disabled as each GPU job requires one CPU thread as well
    -j, --cuda-jobs <jobs>     Parallel jobs per GPU, defaults to 1
                               Example: -g 0 -g 1 -j 4
                               The above example will result in 8 jobs in total
    -p, --pool <pool>          Pool address:port
    -t, --threads <threads>    Number of threads
```

Use your own address as the prover address, not the pool's address.

## Optimizations

You can specify the number of threads to use by `--threads` option. The prover will use all available threads by default, but it won't use 100% of them all the time. You can try adjusting the number to see if using more threads gives better proof rate.

You can enable debug logging by `--debug` option. When starting the prover, there will be a debug log which outputs the structure of the thread pools.

## GPU support?

GPU support is added in version 0.2.0.

Use `-g` option to enable GPU support. To use multiple GPUs, use `-g` multiple times with different GPU indexes.

Use `-j` option to specify the number of jobs per GPU. The default is 1.

Every GPU job will use a CPU thread as well, so it's really a "GPU accelerated prover" instead of a "GPU prover", as only the scalar multiplication on BLS12-377 curve is GPU accelerated.

snarkVM would load programs to all GPUs in the system but the prover will only use the specified GPUs. It wastes some GPU memory, unfortunately 

## License

GPL-3.0-or-later