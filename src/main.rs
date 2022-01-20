mod node;
mod prover;

use snarkvm::dpc::testnet2::Testnet2;
use snarkvm::dpc::Address;
use std::net::SocketAddr;
use std::sync::Arc;
use structopt::StructOpt;

use crate::node::{start, Node};
use crate::prover::Prover;
#[cfg(vanity)]
use snarkvm::dpc::{Account, AccountScheme};
use tracing::{debug, error, info};

#[derive(Debug, StructOpt)]
#[structopt(name = "prover", about = "Standalone prover.", setting = structopt::clap::AppSettings::ColoredHelp)]
struct Opt {
    /// Enable debug logging
    #[structopt(short = "d", long = "debug")]
    debug: bool,

    /// Prover address (aleo1...)
    #[structopt(short = "a", long = "address")]
    address: Address<Testnet2>,

    /// Pool address:port
    #[structopt(short = "p", long = "pool")]
    pool: SocketAddr,

    /// Number of threads
    #[structopt(short = "t", long = "threads")]
    threads: Option<u16>,

    #[cfg(feature = "enable-cuda")]
    #[structopt(verbatim_doc_comment)]
    /// Indexes of GPUs to use (starts from 0)
    /// Specify multiple times to use multiple GPUs
    /// Example: -g 0 -g 1 -g 2
    /// Note: CPU proving will be disabled
    #[structopt(short = "g", long = "cuda")]
    cuda: Option<Vec<i16>>,

    #[cfg(feature = "enable-cuda")]
    #[structopt(verbatim_doc_comment)]
    /// Parallel jobs per GPU, defaults to 1
    /// Example: -g 0 -g 1 -j 4 will result in 8 jobs in total
    /// Each job will use one CPU core as well
    #[structopt(short = "j", long = "cuda-jobs")]
    jobs: Option<u8>,
}

#[tokio::main]
async fn main() {
    let opt = Opt::from_args();
    let _ = tracing_subscriber::fmt()
        .with_ansi(true)
        .with_writer(std::io::stdout)
        .with_max_level(if opt.debug {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        })
        .try_init();
    let threads = opt.threads.unwrap_or(num_cpus::get() as u16);
    let cuda: Option<Vec<i16>>;
    let cuda_jobs: Option<u8>;
    #[cfg(feature = "enable-cuda")]
    {
        cuda = opt.cuda;
        cuda_jobs = opt.jobs
    }
    #[cfg(not(feature = "enable-cuda"))]
    {
        cuda = None;
        cuda_jobs = None;
    }

    info!("Starting prover");
    let node = Node::init(opt.address, opt.pool);
    debug!("Node initialized");

    let prover: Arc<Prover> =
        match Prover::init(opt.address, threads, node.clone(), cuda, cuda_jobs).await {
            Ok(prover) => prover,
            Err(e) => {
                error!("Unable to initialize prover: {}", e);
                std::process::exit(1);
            }
        };
    debug!("Prover initialized");

    start(prover.router(), node.clone(), node.receiver());

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
