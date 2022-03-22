#[forbid(unsafe_code)]
mod client;
mod message;
mod prover;

use std::{net::ToSocketAddrs, sync::Arc};

use snarkvm::dpc::{testnet2::Testnet2, Account, Address};
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;

use crate::{
    client::{start, Client},
    prover::Prover,
};
use aleo_prover::Opt;
use structopt::StructOpt;


#[tokio::main]
async fn main() {
    #[cfg(windows)]
    let _ = ansi_term::enable_ansi_support();

    let opt = Opt::from_args();
    opt.start_mine().await;
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
