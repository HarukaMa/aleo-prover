use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use ansi_term::Colour::{Cyan, Green, Red};
use anyhow::Result;
use rand::thread_rng;
use rayon::{ThreadPool, ThreadPoolBuilder};
use snarkvm::dpc::{testnet2::Testnet2, BlockHeader, BlockTemplate};
use tokio::{sync::mpsc, task};
use tracing::{debug, error, info};

use crate::{message::ProverMessage, Client};

#[derive(Clone)]
pub struct Prover {
    /// thread_pools := Vec<(terminator, ready, threadpool)>
    /// TODO: refactor to a struct
    thread_pools: Arc<Vec<(Arc<AtomicBool>, Arc<AtomicBool>, Arc<ThreadPool>)>>,
    cuda: Option<Vec<i16>>,
    #[allow(dead_code)]
    cuda_jobs: Option<u8>,
    sender: Arc<mpsc::Sender<ProverEvent>>,
    client: Arc<Client>,
    current_block: Arc<AtomicU32>,
    total_proofs: Arc<AtomicU32>,
    valid_shares: Arc<AtomicU32>,
    invalid_shares: Arc<AtomicU32>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum ProverEvent {
    NewWork(u64, BlockTemplate<Testnet2>),
    Result(bool, Option<String>),
}

impl Prover {
    pub async fn init(
        threads: u16,
        client: Arc<Client>,
        cuda: Option<Vec<i16>>,
        cuda_jobs: Option<u8>,
    ) -> Result<Arc<Self>> {
        let mut thread_pools = Vec::new();
        if cuda.is_none() {
            let pool_count;
            let pool_threads;
            if threads % 12 == 0 {
                pool_count = threads / 12;
                pool_threads = 12;
            } else if threads % 10 == 0 {
                pool_count = threads / 10;
                pool_threads = 10;
            } else if threads % 8 == 0 {
                pool_count = threads / 8;
                pool_threads = 8;
            } else {
                pool_count = threads / 6;
                pool_threads = 6;
            }
            for index in 0..pool_count {
                let pool = ThreadPoolBuilder::new()
                    .stack_size(8 * 1024 * 1024)
                    .num_threads(pool_threads as usize)
                    .thread_name(move |idx| format!("ap-cpu-{}-{}", index, idx))
                    .build()?;
                thread_pools.push((
                    Arc::new(AtomicBool::new(false)),
                    // initially, pools are ready to go
                    Arc::new(AtomicBool::new(true)),
                    Arc::new(pool),
                ));
            }
            info!(
                "Created {} prover thread pools with {} threads each",
                thread_pools.len(),
                pool_threads
            );
        } else {
            let total_jobs = cuda_jobs.unwrap_or(1) * cuda.clone().unwrap().len() as u8;
            for index in 0..total_jobs {
                let pool = ThreadPoolBuilder::new()
                    .stack_size(8 * 1024 * 1024)
                    .num_threads(2)
                    .thread_name(move |idx| format!("ap-cuda-{}-{}", index, idx))
                    .build()?;
                thread_pools.push((
                    Arc::new(AtomicBool::new(false)),
                    // initially, pools are ready to go
                    Arc::new(AtomicBool::new(true)),
                    Arc::new(pool),
                ));
            }
            info!("Created {} prover thread pools with 2 threads each", thread_pools.len(),);
        }

        let (sender, mut receiver) = mpsc::channel(1024);
        let prover = Arc::new(Self {
            thread_pools: Arc::new(thread_pools),
            cuda,
            cuda_jobs,
            sender: Arc::new(sender),
            client,
            current_block: Default::default(),
            total_proofs: Default::default(),
            valid_shares: Default::default(),
            invalid_shares: Default::default(),
        });

        let p = prover.clone();
        let _ = task::spawn(async move {
            while let Some(msg) = receiver.recv().await {
                match msg {
                    ProverEvent::NewWork(difficulty, block_template) => {
                        p.new_work(difficulty, block_template).await;
                    }
                    ProverEvent::Result(success, error) => {
                        p.result(success, error).await;
                    }
                }
            }
        });
        debug!("Created prover message handler");

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
                let proofs = total_proofs.load(Ordering::SeqCst);
                log.push_back(proofs);
                let m1 = *log.get(59).unwrap_or(&0);
                let m5 = *log.get(55).unwrap_or(&0);
                let m15 = *log.get(45).unwrap_or(&0);
                let m30 = *log.get(30).unwrap_or(&0);
                let m60 = log.pop_front().unwrap_or_default();
                info!(
                    "{}",
                    Cyan.normal().paint(format!(
                        "Total proofs: {} (1m: {} p/s, 5m: {} p/s, 15m: {} p/s, 30m: {} p/s, 60m: {} p/s)",
                        proofs,
                        calculate_proof_rate(proofs, m1, 1),
                        calculate_proof_rate(proofs, m5, 5),
                        calculate_proof_rate(proofs, m15, 15),
                        calculate_proof_rate(proofs, m30, 30),
                        calculate_proof_rate(proofs, m60, 60),
                    ))
                );
            }
        });
        debug!("Created proof rate calculator");

        Ok(prover)
    }

    pub fn sender(&self) -> Arc<mpsc::Sender<ProverEvent>> {
        self.sender.clone()
    }

    async fn result(&self, success: bool, msg: Option<String>) {
        if success {
            let valid_minus_1 = self.valid_shares.fetch_add(1, Ordering::SeqCst);
            let valid = valid_minus_1 + 1;
            let invalid = self.invalid_shares.load(Ordering::SeqCst);
            if let Some(msg) = msg {
                info!(
                    "{}",
                    Green.normal().paint(format!(
                        "Share accepted: {}  {} / {} ({:.2}%)",
                        msg,
                        valid,
                        valid + invalid,
                        (valid as f64 / (valid + invalid) as f64) * 100.0
                    ))
                );
            } else {
                info!(
                    "{}",
                    Green.normal().paint(format!(
                        "Share accepted  {} / {} ({:.2}%)",
                        valid,
                        valid + invalid,
                        (valid as f64 / (valid + invalid) as f64) * 100.0
                    ))
                );
            }
        } else {
            let invalid_minus_1 = self.invalid_shares.fetch_add(1, Ordering::SeqCst);
            let invalid = invalid_minus_1 + 1;
            let valid = self.valid_shares.load(Ordering::SeqCst);
            if let Some(msg) = msg {
                info!(
                    "{}",
                    Red.normal().paint(format!(
                        "Share rejected: {}  {} / {} ({:.2}%)",
                        msg,
                        valid,
                        valid + invalid,
                        (valid as f64 / (valid + invalid) as f64) * 100.0
                    ))
                );
            } else {
                info!(
                    "{}",
                    Red.normal().paint(format!(
                        "Share rejected  {} / {} ({:.2}%)",
                        valid,
                        valid + invalid,
                        (valid as f64 / (valid + invalid) as f64) * 100.0
                    ))
                );
            }
        }
    }

    async fn new_work(&self, share_difficulty: u64, block_template: BlockTemplate<Testnet2>) {
        let block_template = Arc::new(block_template);
        let block_height = block_template.block_height();
        self.current_block.store(block_height, Ordering::SeqCst);
        info!(
            "Received new work: block {}, share target {}",
            block_height,
            u64::MAX / share_difficulty
        );

        // Terminate all running pools(i.e., ready is false)
        for pool in self.thread_pools.iter() {
            if !pool.1.load(Ordering::SeqCst) {
                pool.0.store(true, Ordering::SeqCst);
            }
        }

        for (idx, pool) in self.thread_pools.iter().enumerate() {
            let block_template = block_template.clone();
            let cuda = self.cuda.clone();

            let prover = self.clone();
            let pool = pool.clone();
            let ready = pool.1.clone();
            task::spawn(async move {
                let start = Instant::now();
                // Wait for the pool to become ready to run new work
                while !ready.load(Ordering::SeqCst) {
                    // waiting won't be very long, using spin_loop to save energy
                    // may be yield current thread is a better choice, not tested yet.
                    std::hint::spin_loop();
                }
                debug!(
                    "stale work terminated, elapsed: {}ms, height: {}",
                    start.elapsed().as_millis(),
                    block_template.block_height()
                );
                // The pool is ready now
                pool.0.store(false, Ordering::SeqCst);

                if let Some(cuda) = cuda {
                    let gpu_index = cuda.get(idx % cuda.len()).unwrap().clone();
                    prover
                        .work_in_pool_gpu(block_template, pool, share_difficulty, gpu_index)
                        .await;
                } else {
                    prover.work_in_pool_cpu(block_template, pool, share_difficulty).await;
                }
            });
        }
    }

    // fn start_pools(&self, share_difficulty: u64, block_template: BlockTemplate<Testnet2>) {}

    async fn work_in_pool_cpu(
        &self,
        block_template: Arc<BlockTemplate<Testnet2>>,
        pool: (Arc<AtomicBool>, Arc<AtomicBool>, Arc<ThreadPool>),
        share_difficulty: u64,
    ) {
        self.work_in_pool_gpu(block_template, pool, share_difficulty, -1).await;
    }

    // TODO: refactor
    async fn work_in_pool_gpu(
        &self,
        block_template: Arc<BlockTemplate<Testnet2>>,
        pool: (Arc<AtomicBool>, Arc<AtomicBool>, Arc<ThreadPool>),
        share_difficulty: u64,
        gpu_index: i16,
    ) {
        let current_block = self.current_block.clone();
        let block_height = block_template.block_height();
        let client = self.client.clone();
        let total_proofs = self.total_proofs.clone();
        let (terminator, ready, pool) = pool;

        // Work until the pool receive a new block.
        while !terminator.load(Ordering::SeqCst) {
            // Set "ready" to false to indicate that pool is working
            ready.store(false, Ordering::SeqCst);
            // Ensure current work is up to date before installing work in pool.
            if block_height != current_block.load(Ordering::SeqCst) {
                debug!(
                    "Terminating stale work before install: current {} latest {}",
                    block_height,
                    current_block.load(Ordering::SeqCst)
                );
                // The pool is ready for new work now
                ready.store(true, Ordering::SeqCst);
                break;
            }
            if gpu_index != -1 {
                debug!("Spawning CUDA thread on GPU {}", gpu_index);
            }

            let pool = pool.clone();
            let terminator = terminator.clone();
            let block_template = block_template.clone();
            if let Ok(Ok(block_header)) = task::spawn_blocking(move || {
                pool.install(|| {
                    BlockHeader::mine_once_unchecked(&block_template, &terminator, &mut thread_rng(), gpu_index)
                })
            })
            .await
            {
                if block_height != current_block.load(Ordering::SeqCst) {
                    debug!(
                        "Terminating stale work after computing: current {} latest {}",
                        block_height,
                        current_block.load(Ordering::SeqCst)
                    );
                    ready.store(true, Ordering::SeqCst);
                    break;
                }
                // Ensure the share difficulty target is met.
                let nonce = block_header.nonce();
                let proof = block_header.proof().clone();
                let proof_difficulty = proof.to_proof_difficulty().unwrap_or(u64::MAX);
                if proof_difficulty > share_difficulty {
                    debug!(
                        "Share difficulty target not met: {} > {}",
                        proof_difficulty, share_difficulty
                    );
                    total_proofs.fetch_add(1, Ordering::SeqCst);
                    continue;
                }

                info!(
                    "Share found for block {} with weight {}",
                    block_height,
                    u64::MAX / proof_difficulty
                );

                // Send a `PoolResponse` to the operator.
                let message = ProverMessage::Submit(block_height, nonce, proof);
                if let Err(error) = client.sender().send(message).await {
                    error!("Failed to send PoolResponse: {}", error);
                }
                total_proofs.fetch_add(1, Ordering::SeqCst);
            } else {
                // Stale work has been terminated, the pool is ready now.
                ready.store(true, Ordering::SeqCst);
                info!("pool cleaned, stale height {}", block_height);
                break;
            }
        }
    }
}
