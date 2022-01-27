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
use tracing_subscriber::layer::SubscriberExt;

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

    /// Output log to file
    #[structopt(short = "o", long = "log")]
    log: Option<String>,

    #[cfg(feature = "enable-cuda")]
    #[structopt(verbatim_doc_comment)]
    /// Indexes of GPUs to use (starts from 0)
    /// Specify multiple times to use multiple GPUs
    /// Example: -g 0 -g 1 -g 2
    /// Note: Pure CPU proving will be disabled as each GPU job requires one CPU thread as well
    #[structopt(short = "g", long = "cuda")]
    cuda: Option<Vec<i16>>,

    #[cfg(feature = "enable-cuda")]
    #[structopt(verbatim_doc_comment)]
    /// Parallel jobs per GPU, defaults to 1
    /// Example: -g 0 -g 1 -j 4
    /// The above example will result in 8 jobs in total
    #[structopt(short = "j", long = "cuda-jobs")]
    jobs: Option<u8>,

    #[cfg(feature = "enable-cuda")]
    #[structopt(verbatim_doc_comment)]
    /// CPU threads per GPU job, defaults to 1
    /// Example: -g 0 -g 1 -j 4 -c 2
    /// The above example will use 16 CPU threads in total
    #[structopt(short = "c", long = "cuda-cpu-threads")]
    cuda_cpu: Option<u8>,
}

#[tokio::main]
async fn main() {
    let opt = Opt::from_args();
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
        let file = tracing_subscriber::fmt::layer()
            .with_writer(file)
            .with_ansi(false);
        tracing::subscriber::set_global_default(subscriber.with(file))
            .expect("unable to set global default subscriber");
    } else {
        tracing::subscriber::set_global_default(subscriber)
            .expect("unable to set global default subscriber");
    }

    let threads = opt.threads.unwrap_or(num_cpus::get() as u16);

    let cuda: Option<Vec<i16>>;
    let cuda_jobs: Option<u8>;
    let cuda_cpu_threads: Option<u8>;
    #[cfg(feature = "enable-cuda")]
    {
        cuda = opt.cuda;
        cuda_jobs = opt.jobs;
        cuda_cpu_threads = opt.cuda_cpu;
    }
    #[cfg(not(feature = "enable-cuda"))]
    {
        cuda = None;
        cuda_jobs = None;
        cuda_cpu_threads = None;
    }
    if let Some(cuda) = cuda.clone() {
        if cuda.is_empty() {
            error!("No GPUs specified. Use -g 0 if there is only one GPU.");
            std::process::exit(1);
        }
    }

    info!("Starting prover");
    let node = Node::init(opt.address, opt.pool);
    debug!("Node initialized");

    let prover: Arc<Prover> = match Prover::init(
        opt.address,
        threads,
        node.clone(),
        cuda,
        cuda_jobs,
        cuda_cpu_threads,
    )
    .await
    {
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
