use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

use ansi_term::Colour::{Cyan, Green, Red};
use anyhow::Result;
use rand::thread_rng;
use rayon::{ThreadPool, ThreadPoolBuilder};
use snarkvm::dpc::{testnet2::Testnet2, BlockHeader, BlockTemplate};
use tokio::{sync::mpsc, task};
use tracing::{debug, error, info};

use crate::{message::ProverMessage, Client};

pub struct Prover {
    thread_pools: Arc<Vec<Arc<ThreadPool>>>,
    cuda: Option<Vec<i16>>,
    cuda_jobs: Option<u8>,
    sender: Arc<mpsc::Sender<ProverEvent>>,
    client: Arc<Client>,
    terminator: Arc<AtomicBool>,
    current_block: Arc<AtomicU32>,
    total_proofs: Arc<AtomicU32>,
    valid_shares: Arc<AtomicU32>,
    invalid_shares: Arc<AtomicU32>,
}

#[allow(clippy::large_enum_variant)]
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
        let mut thread_pools: Vec<Arc<ThreadPool>> = Vec::new();
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
        if cuda.is_none() {
            for index in 0..pool_count {
                let pool = ThreadPoolBuilder::new()
                    .stack_size(8 * 1024 * 1024)
                    .num_threads(pool_threads as usize)
                    .thread_name(move |idx| format!("ap-cpu-{}-{}", index, idx))
                    .build()?;
                thread_pools.push(Arc::new(pool));
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
                thread_pools.push(Arc::new(pool));
            }
            info!("Created {} prover thread pools with 2 threads each", thread_pools.len(),);
        }

        let (sender, mut receiver) = mpsc::channel(1024);
        let terminator = Arc::new(AtomicBool::new(false));
        let prover = Arc::new(Self {
            thread_pools: Arc::new(thread_pools),
            cuda,
            cuda_jobs,
            sender: Arc::new(sender),
            client,
            terminator,
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
        let block_height = block_template.block_height();
        self.current_block.store(block_height, Ordering::SeqCst);
        info!(
            "Received new work: block {}, share target {}",
            block_template.block_height(),
            u64::MAX / share_difficulty
        );

        let current_block = self.current_block.clone();
        let terminator = self.terminator.clone();
        let client = self.client.clone();
        let thread_pools = self.thread_pools.clone();
        let total_proofs = self.total_proofs.clone();
        let cuda = self.cuda.clone();
        let cuda_jobs = self.cuda_jobs;

        task::spawn(async move {
            terminator.store(true, Ordering::SeqCst);
            while terminator.load(Ordering::SeqCst) {
                // Wait until the prover terminator is set to false.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            let _ = task::spawn(async move {
                let mut joins = Vec::new();
                if let Some(cuda) = cuda {
                    for gpu_index in cuda {
                        for job_index in 0..cuda_jobs.unwrap_or(1) {
                            let tp = thread_pools
                                .get(gpu_index as usize * cuda_jobs.unwrap_or(1) as usize + job_index as usize)
                                .unwrap();
                            debug!("Spawning CUDA thread on GPU {} job {}", gpu_index, job_index,);
                            work_with_gpu(
                                &current_block,
                                &terminator,
                                &client,
                                &block_template,
                                &total_proofs,
                                tp,
                                &mut joins,
                                gpu_index,
                                share_difficulty,
                            );
                        }
                    }
                } else {
                    for tp in thread_pools.iter() {
                        work_with_cpu(
                            &current_block,
                            &terminator,
                            &client,
                            &block_template,
                            &total_proofs,
                            tp,
                            &mut joins,
                            share_difficulty,
                        );
                    }
                }
                futures::future::join_all(joins).await;
                terminator.store(false, Ordering::SeqCst);
            });
        });
    }
}

fn work_with_cpu(
    current_block: &Arc<AtomicU32>,
    terminator: &Arc<AtomicBool>,
    client: &Arc<Client>,
    block_template: &BlockTemplate<Testnet2>,
    total_proofs: &Arc<AtomicU32>,
    tp: &Arc<ThreadPool>,
    joins: &mut Vec<task::JoinHandle<()>>,
    share_difficulty: u64,
) {
    work_with_gpu(
        current_block,
        terminator,
        client,
        block_template,
        total_proofs,
        tp,
        joins,
        -1,
        share_difficulty,
    )
}

fn work_with_gpu(
    current_block: &Arc<AtomicU32>,
    terminator: &Arc<AtomicBool>,
    client: &Arc<Client>,
    block_template: &BlockTemplate<Testnet2>,
    total_proofs: &Arc<AtomicU32>,
    tp: &Arc<ThreadPool>,
    joins: &mut Vec<task::JoinHandle<()>>,
    gpu_index: i16,
    share_difficulty: u64,
) {
    let current_block = current_block.clone();
    let terminator = terminator.clone();
    let client = client.clone();
    let block_template = block_template.clone();
    let total_proofs = total_proofs.clone();
    let tp = tp.clone();
    joins.push(task::spawn(async move {
        while !terminator.load(Ordering::SeqCst) {
            let terminator = terminator.clone();
            let block_template = block_template.clone();
            let block_height = block_template.block_height();
            let tp = tp.clone();
            if block_height != current_block.load(Ordering::SeqCst) {
                debug!(
                    "Terminating stale work: current {} latest {}",
                    block_height,
                    current_block.load(Ordering::SeqCst)
                );
                break;
            }
            if let Ok(Ok(block_header)) = task::spawn_blocking(move || {
                tp.install(|| {
                    BlockHeader::mine_once_unchecked(&block_template, &terminator, &mut thread_rng(), gpu_index)
                })
            })
            .await
            {
                if block_height != current_block.load(Ordering::SeqCst) {
                    debug!(
                        "Terminating stale work: current {} latest {}",
                        block_height,
                        current_block.load(Ordering::SeqCst)
                    );
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
            }
        }
    }));
}
