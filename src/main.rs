extern crate core;

#[forbid(unsafe_code)]
mod client;
mod prover;

use std::{net::ToSocketAddrs, sync::Arc};

use clap::Parser;
use snarkvm::{
    console::account::address::Address,
    prelude::{PrivateKey, Testnet3, ViewKey},
};
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;

use crate::{
    client::{start, Client},
    prover::Prover,
};

#[derive(Debug, Parser)]
#[clap(name = "prover", about = "Standalone prover.")]
struct Opt {
    /// Enable debug logging
    #[clap(short = 'd', long = "debug")]
    debug: bool,

    /// Prover address (aleo1...)
    #[clap(short = 'a', long = "address")]
    address: Option<Address<Testnet3>>,

    /// Pool server address
    #[clap(short = 'p', long = "pool")]
    pool: Option<String>,

    /// Number of threads, defaults to number of CPU threads
    #[clap(short = 't', long = "threads")]
    threads: Option<u16>,

    /// Thread pool size, number of threads in each thread pool, defaults to 4
    #[clap(short = 'i', long = "thread-pool-size")]
    thread_pool_size: Option<u8>,

    /// Output log to file
    #[clap(short = 'o', long = "log")]
    log: Option<String>,

    /// Generate a new address
    #[clap(long = "new-address")]
    new_address: bool,

    #[cfg(feature = "cuda")]
    #[clap(verbatim_doc_comment)]
    /// Indexes of GPUs to use (starts from 0)
    /// Specify multiple times to use multiple GPUs
    /// Example: -g 0 -g 1 -g 2
    /// Note: Pure CPU proving will be disabled as each GPU job requires one CPU thread as well
    #[clap(short = 'g', long = "cuda")]
    cuda: Option<Vec<i16>>,

    #[cfg(feature = "cuda")]
    #[clap(verbatim_doc_comment)]
    /// Parallel jobs per GPU, defaults to 1
    /// Example: -g 0 -g 1 -j 4
    /// The above example will result in 8 jobs in total
    #[clap(short = 'j', long = "cuda-jobs")]
    jobs: Option<u8>,
}

#[tokio::main]
async fn main() {
    #[cfg(windows)]
    let _ = ansi_term::enable_ansi_support();

    let opt = Opt::parse();
    if opt.new_address {
        let private_key = PrivateKey::<Testnet3>::new(&mut rand::thread_rng()).unwrap();
        let view_key = ViewKey::try_from(&private_key).unwrap();
        let address = Address::try_from(&view_key).unwrap();
        println!();
        println!("Private key: {}", private_key);
        println!("   View key: {}", view_key);
        println!("    Address: {}", address);
        println!();
        println!("WARNING: Make sure you have a backup of both private key and view key!");
        println!("         Nobody can help you recover those keys if you lose them!");
        println!();
        return;
    }
    let tracing_level = if opt.debug {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_max_level(tracing_level)
        .finish();
    // .with(
    //     tracing_subscriber::fmt::Layer::default()
    //         .with_ansi(true)
    //         .with_writer(std::io::stdout),
    // );
    if let Some(log) = opt.log {
        let file = std::fs::File::create(log).unwrap();
        let file = tracing_subscriber::fmt::layer().with_writer(file).with_ansi(false);
        tracing::subscriber::set_global_default(subscriber.with(file))
            .expect("unable to set global default subscriber");
    } else {
        tracing::subscriber::set_global_default(subscriber).expect("unable to set global default subscriber");
    }

    if opt.pool.is_none() {
        error!("Pool address is required!");
    }
    if opt.address.is_none() {
        error!("Prover address is required!");
        std::process::exit(1);
    }
    let address = opt.address.unwrap();
    let pool = opt.pool.unwrap();
    if let Err(e) = pool.to_socket_addrs() {
        error!("Invalid pool address {}: {}", pool, e);
        std::process::exit(1);
    }

    let threads = opt.threads.unwrap_or(num_cpus::get() as u16);
    let thread_pool_size = opt.thread_pool_size.unwrap_or(4);

    let cuda: Option<Vec<i16>>;
    let cuda_jobs: Option<u8>;
    #[cfg(feature = "cuda")]
    {
        cuda = opt.cuda;
        cuda_jobs = opt.jobs;
    }
    #[cfg(not(feature = "cuda"))]
    {
        cuda = None;
        cuda_jobs = None;
    }
    if let Some(cuda) = cuda.clone() {
        if cuda.is_empty() {
            error!("No GPUs specified. Use -g 0 if there is only one GPU.");
            std::process::exit(1);
        }
    }

    info!("Starting prover");
    // if opt.old_protocol {
    //     info!("Using old protocol");
    //     let node = Node::init(address, pool);
    //     debug!("Node initialized");
    // }

    let client = Client::init(address, pool);

    let prover: Arc<Prover> = match Prover::init(threads, thread_pool_size, client.clone(), cuda, cuda_jobs).await {
        Ok(prover) => prover,
        Err(e) => {
            error!("Unable to initialize prover: {}", e);
            std::process::exit(1);
        }
    };
    debug!("Prover initialized");

    start(prover.sender(), client.clone());

    std::future::pending::<()>().await;
}

#[cfg(vanity)]
async fn vanity() {
    let count: Arc<RwLock<u32>> = Arc::new(RwLock::new(0u32));
    let end: Arc<RwLock<bool>> = Arc::new(RwLock::new(false));
    let mut threads = vec![];
    for _ in 0..num_cpus::get() {
        let count = count.clone();
        let end = end.clone();
        threads.push(task::spawn(async move {
            loop {
                for _ in 0..100 {
                    let account = Account::<Testnet2>::new(&mut rand::thread_rng());
                    if format!("{}", account.address()).starts_with("aleo1haruka") {
                        println!("{}", account.private_key());
                        println!("{}", account.view_key());
                        println!("{}", account.address());
                        *end.write().unwrap() = true;
                    }
                }
                *count.write().unwrap() += 100;
                let c = *count.read().unwrap();
                println!("{}", c);
                if *end.read().unwrap() {
                    break;
                }
            }
        }));
    }
    for t in threads {
        // Wait for the thread to finish. Returns a result.
        t.await;
    }
}
