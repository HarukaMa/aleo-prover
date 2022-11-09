# Aleo Light Prover

# "Testnet 3 direct" branch

The prover can directly connect to the beacon node and solve coinbase puzzles using the old thread pool config.

The document below is outdated and might not apply to this branch. Use `--help` to see how to run the prover.

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
prover 0.3.0
Standalone prover.

USAGE:
    aleo-prover [FLAGS] [OPTIONS] --address <address> --pool <pool>

FLAGS:
    -d, --debug          Enable debug logging
    -h, --help           Prints help information
        --new-address    Generate a new address
    -V, --version        Prints version information

OPTIONS:
    -a, --address <address>    Prover address (aleo1...)
    -g, --cuda <cuda>...       Indexes of GPUs to use (starts from 0)
                               Specify multiple times to use multiple GPUs
                               Example: -g 0 -g 1 -g 2
                               Note: Pure CPU proving will be disabled as each GPU job requires one CPU thread as well
    -j, --cuda-jobs <jobs>     Parallel jobs per GPU, defaults to 1
                               Example: -g 0 -g 1 -j 4
                               The above example will result in 8 jobs in total
    -o, --log <log>            Output log to file
    -p, --pool <pool>          Pool server address
    -t, --threads <threads>    Number of threads
```

Use your own address as the prover address, not the pool's address.

## Optimizations

You can specify the number of threads to use by `--threads` option. The prover will use all available threads by default.

Starting form version 0.3.0, you generally don't need to adjust the number of threads.

You can enable debug logging by `--debug` option. 

## GPU support?

GPU support is added in version 0.2.0.

To enable GPU support, use `cargo build --release --features cuda` when building the binary. Obviously you will need to install the CUDA runtime.

Use `-g` option to enable GPU support. To use multiple GPUs, use `-g` multiple times with different GPU indexes.

Use `-j` option to specify the number of jobs per GPU. The default is 1.

Every GPU job will use a CPU thread as well, so it's really a "GPU accelerated prover" instead of a "GPU prover", as only the scalar multiplication on BLS12-377 curve is GPU accelerated.

snarkVM would load programs to all GPUs in the system but the prover will only use the specified GPUs. It wastes some GPU memory, unfortunately.

## Changelog

### 0.3.0
Added support for the new pool server.  
Dropped support for directly connecting to operator nodes.  
Fixed a bug that caused the prover to block the networking thread.  

### 0.2.7
GPU proving should be slightly faster (~20%).

### 0.2.6
You can now generate new Aleo addresses by using `--new-address` option.  
You can use domain names when specifying the pool address.

### 0.2.5
Stopped the prover from sending stale shares to pool.  
Note that you might see the speed reported by the prover to drop a little. However, the actual valid shares are not affected. 

### 0.2.4
GPU proving should be slightly faster (~5-10%).

### 0.2.3
Changed the thread pool configuration for CPU proving.  
Promoted the thread pool configuration log to info level.

### 0.2.2
Added log file support.  
Added extra check for `-g` option.

### 0.2.1
Removed OpenCL dependency.

### 0.2.0
Added GPU support.

### 0.1.0
Initial release.

## License

GPL-3.0-or-later
