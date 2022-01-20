use crate::node::SendMessage;
use crate::Node;
use anyhow::Result;
use crossbeam::sync::WaitGroup;
use futures::executor::block_on;
use rand::thread_rng;
use rayon::{ThreadPool, ThreadPoolBuilder};
use snarkos::{Data, Message};
use snarkvm::dpc::testnet2::Testnet2;
use snarkvm::dpc::{Address, BlockHeader, BlockTemplate};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::{sync::mpsc, task};
use tracing::{debug, error, info};

pub struct Prover {
    address: Address<Testnet2>,
    thread_pools: Arc<Vec<ThreadPool>>,
    cuda: Option<Vec<i16>>,
    cuda_jobs: Option<u8>,
    router: Arc<mpsc::Sender<ProverWork>>,
    node: Arc<Node>,
    terminator: Arc<AtomicBool>,
    current_block: Arc<RwLock<u32>>,
    total_proofs: Arc<RwLock<u32>>,
}

#[derive(Debug)]
pub struct ProverWork {
    share_difficulty: u64,
    block_template: BlockTemplate<Testnet2>,
}

impl ProverWork {
    pub fn new(share_difficulty: u64, block_template: BlockTemplate<Testnet2>) -> Self {
        Self {
            share_difficulty,
            block_template,
        }
    }
}

impl Prover {
    pub async fn init(
        address: Address<Testnet2>,
        threads: u16,
        node: Arc<Node>,
        cuda: Option<Vec<i16>>,
        cuda_jobs: Option<u8>,
    ) -> Result<Arc<Self>> {
        let mut thread_pools: Vec<ThreadPool> = Vec::new();
        let pool_count;
        let pool_threads;
        if threads % 12 == 0 {
            pool_count = threads / 12;
            pool_threads = 12;
        } else if threads % 8 == 0 {
            pool_count = threads / 8;
            pool_threads = 8;
        } else if threads % 6 == 0 {
            pool_count = threads / 6;
            pool_threads = 6;
        } else {
            pool_count = threads / 4;
            pool_threads = 4;
        };
        if !cfg!(feature = "enable-cuda") || cuda.is_none() {
            for _ in 0..pool_count {
                let pool = ThreadPoolBuilder::new()
                    .stack_size(8 * 1024 * 1024)
                    .num_threads(pool_threads as usize)
                    .build()?;
                thread_pools.push(pool);
            }
            debug!(
                "Created {} prover thread pools with {} threads each",
                thread_pools.len(),
                pool_threads
            );
        } else {
            let total_jobs = cuda_jobs.clone().unwrap_or(1) * cuda.clone().unwrap().len() as u8;
            for _ in 0..total_jobs {
                let pool = ThreadPoolBuilder::new()
                    .stack_size(8 * 1024 * 1024)
                    .num_threads(1)
                    .build()?;
                thread_pools.push(pool);
            }
        }

        let (router_tx, mut router_rx) = mpsc::channel(1024);
        let terminator = Arc::new(AtomicBool::new(false));
        let prover = Arc::new(Self {
            address,
            thread_pools: Arc::new(thread_pools),
            cuda,
            cuda_jobs,
            router: Arc::new(router_tx),
            node,
            terminator,
            current_block: Default::default(),
            total_proofs: Default::default(),
        });

        let p = prover.clone();
        let _ = task::spawn(async move {
            while let Some(work) = router_rx.recv().await {
                p.new_work(work).await;
            }
        });
        debug!("Created prover message handler");

        let terminator = prover.terminator.clone();

        task::spawn(async move {
            let mut counter = false;
            loop {
                if terminator.load(Ordering::SeqCst) {
                    if counter {
                        debug!("Long terminator detected, resetting");
                        terminator.store(false, Ordering::SeqCst);
                        counter = false;
                    } else {
                        counter = true;
                    }
                } else {
                    counter = false;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        debug!("Created prover terminator guard");

        let total_proofs = prover.total_proofs.clone();
        task::spawn(async move {
            fn calculate_proof_rate(now: u32, past: u32, interval: u32) -> Box<str> {
                if interval < 1 {
                    return Box::from("---");
                }
                if now <= past || past == 0 {
                    return Box::from("---");
                }
                let rate = (now - past) as f64 / (interval * 60) as f64;
                Box::from(format!("{:.2}", rate))
            }
            let mut log = VecDeque::<u32>::from(vec![0; 60]);
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let proofs = *total_proofs.read().await;
                log.push_back(proofs);
                let m1 = *log.get(59).unwrap_or(&0);
                let m5 = *log.get(55).unwrap_or(&0);
                let m15 = *log.get(45).unwrap_or(&0);
                let m30 = *log.get(30).unwrap_or(&0);
                let m60 = log.pop_front().unwrap_or_default();
                info!(
                    "Total proofs: {} (1m: {} p/s, 5m: {} p/s, 15m: {} p/s, 30m: {} p/s, 60m: {} p/s)",
                    proofs,
                    calculate_proof_rate(proofs, m1, 1),
                    calculate_proof_rate(proofs, m5, 5),
                    calculate_proof_rate(proofs, m15, 15),
                    calculate_proof_rate(proofs, m30, 30),
                    calculate_proof_rate(proofs, m60, 60),
                );
            }
        });
        debug!("Created proof rate calculator");

        Ok(prover)
    }

    pub fn router(&self) -> Arc<mpsc::Sender<ProverWork>> {
        self.router.clone()
    }

    async fn new_work(&self, work: ProverWork) {
        let block_template = work.block_template;
        let block_height = block_template.block_height();
        *(self.current_block.write().await) = block_height;
        let share_difficulty = work.share_difficulty;
        info!(
            "Received new work: block {}, share weight {}",
            block_template.block_height(),
            u64::MAX / share_difficulty
        );

        let current_block = self.current_block.clone();
        let terminator = self.terminator.clone();
        let address = self.address;
        let node = self.node.clone();
        let thread_pools = self.thread_pools.clone();
        let total_proofs = self.total_proofs.clone();
        let cuda = self.cuda.clone();
        let cuda_jobs = self.cuda_jobs.clone();

        task::spawn(async move {
            terminator.store(true, Ordering::SeqCst);
            while terminator.load(Ordering::SeqCst) {
                // Wait until the prover terminator is set to false.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            let _ = task::spawn(async move {
                let wg = WaitGroup::new();
                if cfg!(feature = "enable-cuda") && cuda.is_some() {
                    for gpu_index in cuda.unwrap() {
                        for job_index in 0..cuda_jobs.unwrap_or(1) {
                            let tp = thread_pools
                                .get(
                                    gpu_index as usize * cuda_jobs.unwrap_or(1) as usize
                                        + job_index as usize,
                                )
                                .unwrap();
                            let wg = wg.clone();

                            let current_block = current_block.clone();
                            let terminator = terminator.clone();
                            let address = address;
                            let node = node.clone();
                            let block_template = block_template.clone();
                            let total_proofs = total_proofs.clone();
                            tp.spawn(move || {
                                while !terminator.load(Ordering::SeqCst) {
                                    let block_height = block_template.block_height();
                                    if block_height != *(current_block.try_read().unwrap()) {
                                        debug!(
                                            "Terminating stale work: current {} latest {}",
                                            block_height,
                                            *(current_block.try_read().unwrap())
                                        );
                                        break;
                                    }
                                    if let Ok(block_header) = BlockHeader::mine_once_unchecked(
                                        &block_template,
                                        &terminator,
                                        &mut thread_rng(),
                                        gpu_index,
                                    ) {
                                        // Ensure the share difficulty target is met.
                                        let nonce = block_header.nonce();
                                        let proof = block_header.proof().clone();
                                        let proof_difficulty =
                                            proof.to_proof_difficulty().unwrap_or(u64::MAX);
                                        if proof_difficulty > share_difficulty {
                                            debug!(
                                                "Share difficulty target not met: {} > {}",
                                                proof_difficulty, share_difficulty
                                            );
                                            *(block_on(total_proofs.write())) += 1;
                                            continue;
                                        }

                                        info!(
                                            "Share found for block {} with weight {}",
                                            block_height,
                                            u64::MAX / proof_difficulty
                                        );

                                        // Send a `PoolResponse` to the operator.
                                        let message = Message::PoolResponse(
                                            address,
                                            nonce,
                                            Data::Object(proof),
                                        );
                                        if let Err(error) =
                                            block_on(node.router().send(SendMessage { message }))
                                        {
                                            error!("Failed to send PoolResponse: {}", error);
                                        }
                                        *(block_on(total_proofs.write())) += 1;
                                    }
                                }
                                drop(wg);
                            });
                        }
                    }
                } else {
                    for tp in &*thread_pools {
                        let wg = wg.clone();

                        let current_block = current_block.clone();
                        let terminator = terminator.clone();
                        let address = address;
                        let node = node.clone();
                        let block_template = block_template.clone();
                        let total_proofs = total_proofs.clone();
                        tp.spawn(move || {
                            while !terminator.load(Ordering::SeqCst) {
                                let block_height = block_template.block_height();
                                if block_height != *(current_block.try_read().unwrap()) {
                                    debug!(
                                        "Terminating stale work: current {} latest {}",
                                        block_height,
                                        *(current_block.try_read().unwrap())
                                    );
                                    break;
                                }
                                if let Ok(block_header) = BlockHeader::mine_once_unchecked(
                                    &block_template,
                                    &terminator,
                                    &mut thread_rng(),
                                    -1,
                                ) {
                                    // Ensure the share difficulty target is met.
                                    let nonce = block_header.nonce();
                                    let proof = block_header.proof().clone();
                                    let proof_difficulty =
                                        proof.to_proof_difficulty().unwrap_or(u64::MAX);
                                    if proof_difficulty > share_difficulty {
                                        debug!(
                                            "Share difficulty target not met: {} > {}",
                                            proof_difficulty, share_difficulty
                                        );
                                        *(block_on(total_proofs.write())) += 1;
                                        continue;
                                    }

                                    info!(
                                        "Share found for block {} with weight {}",
                                        block_height,
                                        u64::MAX / proof_difficulty
                                    );

                                    // Send a `PoolResponse` to the operator.
                                    let message =
                                        Message::PoolResponse(address, nonce, Data::Object(proof));
                                    if let Err(error) =
                                        block_on(node.router().send(SendMessage { message }))
                                    {
                                        error!("Failed to send PoolResponse: {}", error);
                                    }
                                    *(block_on(total_proofs.write())) += 1;
                                }
                            }
                            drop(wg);
                        })
                    }
                    wg.wait();
                    terminator.store(false, Ordering::SeqCst);
                }
            });
        });
    }
}
