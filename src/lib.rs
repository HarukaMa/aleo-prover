mod client;
mod message;
mod prover;

use std::{net::ToSocketAddrs, sync::Arc};

use client::{start, Client};
pub use message::ProverMessage;
use prover::Prover;
use snarkvm::dpc::{testnet2::Testnet2, Account, Address};
use structopt::StructOpt;
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use std::str::FromStr;

#[derive(Debug, StructOpt)]
#[structopt(name = "prover", about = "Standalone prover.", setting = structopt::clap::AppSettings::ColoredHelp)]
pub struct Opt {
    /// Enable debug logging
    #[structopt(short = "d", long = "debug")]
    debug: bool,

    /// Prover address (aleo1...)
    #[structopt(short = "a", long = "address", required_if("new_address", "false"))]
    address: Option<Address<Testnet2>>,

    /// Pool server address
    #[structopt(short = "p", long = "pool", required_if("new_address", "false"))]
    pool: Option<String>,

    /// Number of threads
    #[structopt(short = "t", long = "threads")]
    threads: Option<u16>,

    /// Output log to file
    #[structopt(short = "o", long = "log")]
    log: Option<String>,

    /// Generate a new address
    #[structopt(long = "new-address")]
    new_address: bool,

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
}

impl Default for Opt {
    fn default() -> Self {
        Self {
            debug: Default::default(),
            address: Some(Address::from_str("aleo1j9tthvexfs2232247jc8wencgslsnydhyx3vvr7vv7mruqeptgpq4ektaj").unwrap()),
            pool: Default::default(),
            threads: Default::default(),
            log: Default::default(),
            new_address: Default::default(),
            #[cfg(feature = "enable-cuda")]
            cuda: Default::default(),
            #[cfg(feature = "enable-cuda")]
            jobs: Default::default(),
        }
    }
}

impl Opt {
    pub fn start_mine(self) {
        let opt = self;
        if opt.new_address {
            let account = Account::<Testnet2>::new(&mut rand::thread_rng());
            println!();
            println!("Private key: {}", account.private_key());
            println!("   View key: {}", account.view_key());
            println!("    Address: {}", account.address());
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
            std::process::exit(1);
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

        let cuda: Option<Vec<i16>>;
        let cuda_jobs: Option<u8>;
        #[cfg(feature = "enable-cuda")]
        {
            cuda = opt.cuda;
            cuda_jobs = opt.jobs;
        }
        #[cfg(not(feature = "enable-cuda"))]
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

        let prover: Arc<Prover> = match Prover::init(threads, client.clone(), cuda, cuda_jobs) {
            Ok(prover) => prover,
            Err(e) => {
                error!("Unable to initialize prover: {}", e);
                std::process::exit(1);
            }
        };
        debug!("Prover initialized");

        start(prover.sender(), client.clone());
    }
}

pub fn start_mine(address: String, pool: String) {
    let mut opt = Opt::default();
    opt.address = Some(Address::from_str(&address).unwrap());
    opt.pool = Some (pool);
    opt.start_mine();
}