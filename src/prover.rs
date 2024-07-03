use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use ansi_term::Colour::{Cyan, Green, Red};
use anyhow::{anyhow, bail, Result};
use futures::executor::block_on;
use rand::{thread_rng, RngCore};
use rayon::{ThreadPool, ThreadPoolBuilder};
use snarkos_node_router_messages::UnconfirmedSolution;
use snarkvm::{
    circuit::AleoTestnetV0,
    ledger::{
        narwhal::Data,
        puzzle::{PartialSolution, Puzzle, PuzzleTrait, Solution},
    },
    prelude::{Address, Network, TestnetV0},
};
use snarkvm_ledger_puzzle_epoch::SynthesisPuzzle;
use tokio::{runtime::Runtime, sync::mpsc, task};
use tracing::{debug, error, info, warn};

use crate::client_direct::DirectClient;

type N = TestnetV0;
type A = AleoTestnetV0;

type Message = snarkos_node_router_messages::Message<N>;

pub struct Prover {
    thread_pools: Arc<Vec<Arc<ThreadPool>>>,
    cuda: Option<Vec<i16>>,
    _cuda_jobs: Option<u8>,
    sender: Arc<mpsc::Sender<ProverEvent>>,
    client: Arc<DirectClient>,
    current_epoch: Arc<AtomicU32>,
    total_proofs: Arc<AtomicU32>,
    valid_shares: Arc<AtomicU32>,
    invalid_shares: Arc<AtomicU32>,
    current_proof_target: Arc<AtomicU64>,
    puzzle: Puzzle<N>,
}

#[allow(clippy::large_enum_variant)]
pub enum ProverEvent {
    NewTarget(u64),
    NewWork(u32, <N as Network>::BlockHash, Address<N>),
    _Result(bool, Option<String>),
}

impl Prover {
    pub async fn init(
        threads: u16,
        thread_pool_size: u8,
        client: Arc<DirectClient>,
        cuda: Option<Vec<i16>>,
        cuda_jobs: Option<u8>,
    ) -> Result<Arc<Self>> {
        let mut thread_pools: Vec<Arc<ThreadPool>> = Vec::new();
        let pool_count;
        let pool_threads;
        if cuda.is_none() {
            if threads < thread_pool_size as u16 {
                pool_count = 1;
                pool_threads = thread_pool_size as u16;
            } else {
                pool_count = threads / thread_pool_size as u16;
                pool_threads = thread_pool_size as u16;
            }
        } else {
            pool_threads = thread_pool_size as u16;
            pool_count = (cuda_jobs.unwrap_or(1) * cuda.clone().unwrap().len() as u8) as u16;
        }
        for index in 0..pool_count {
            let builder = ThreadPoolBuilder::new()
                .stack_size(8 * 1024 * 1024)
                .num_threads(pool_threads as usize);
            let pool = if cuda.is_none() {
                builder.thread_name(move |idx| format!("ap-cpu-{}-{}", index, idx))
            } else {
                builder.thread_name(move |idx| format!("ap-cuda-{}-{}", index, idx))
            }
            .build()?;
            thread_pools.push(Arc::new(pool));
        }
        info!(
            "Created {} prover thread pools with {} threads in each pool",
            thread_pools.len(),
            pool_threads
        );

        let (sender, mut receiver) = mpsc::channel(1024);

        let prover = Arc::new(Self {
            thread_pools: Arc::new(thread_pools),
            cuda,
            _cuda_jobs: cuda_jobs,
            sender: Arc::new(sender),
            client,
            current_epoch: Arc::new(AtomicU32::new(u32::MAX)),
            total_proofs: Default::default(),
            valid_shares: Default::default(),
            invalid_shares: Default::default(),
            current_proof_target: Default::default(),
            puzzle: Puzzle::<N>::new::<SynthesisPuzzle<N, A>>(),
        });

        let p = prover.clone();
        let _ = task::spawn(async move {
            while let Some(msg) = receiver.recv().await {
                match msg {
                    ProverEvent::NewTarget(target) => {
                        p.new_target(target);
                    }
                    ProverEvent::NewWork(epoch_number, epoch_hash, address) => {
                        p.new_work(epoch_number, epoch_hash, address).await;
                    }
                    ProverEvent::_Result(success, error) => {
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
                        "Total solutions: {} (1m: {} c/s, 5m: {} c/s, 15m: {} c/s, 30m: {} c/s, 60m: {} c/s)",
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

    fn new_target(&self, proof_target: u64) {
        self.current_proof_target.store(proof_target, Ordering::SeqCst);
        info!("New proof target: {}", proof_target);
    }

    /// Returns a solution to the puzzle.
    fn prove(
        puzzle: &Puzzle<N>,
        epoch_hash: <N as Network>::BlockHash,
        address: Address<N>,
        counter: u64,
        minimum_proof_target: Option<u64>,
    ) -> Result<(Solution<N>, u64)> {
        let partial = PartialSolution::new(epoch_hash, address, counter)?;
        // Compute the proof target.
        let proof_target = puzzle.get_proof_target_from_partial_solution(&partial)?;
        // Construct the solution.
        let solution = Solution::new(partial, proof_target);
        // Check that the minimum proof target is met.
        if let Some(minimum_proof_target) = minimum_proof_target {
            if proof_target < minimum_proof_target {
                bail!("Solution was below the minimum proof target ({proof_target} < {minimum_proof_target})")
            }
        }
        // Return the solution.
        Ok((solution, proof_target))
    }

    async fn new_work(&self, epoch_number: u32, epoch_hash: <N as Network>::BlockHash, address: Address<N>) {
        let last_epoch_number = self.current_epoch.load(Ordering::SeqCst);
        if last_epoch_number != u32::MAX && epoch_number <= last_epoch_number {
            return;
        }
        self.current_epoch.store(epoch_number, Ordering::SeqCst);
        info!("Received new work: epoch {}", epoch_number);
        let current_proof_target = self.current_proof_target.clone();

        let current_epoch = self.current_epoch.clone();
        let client = self.client.clone();
        let thread_pools = self.thread_pools.clone();
        let total_proofs = self.total_proofs.clone();
        let puzzle = self.puzzle.clone();

        for (i, _) in thread_pools.iter().enumerate() {
            let current_proof_target = current_proof_target.clone();
            let current_epoch = current_epoch.clone();
            let client = client.clone();
            let epoch_hash = epoch_hash.clone();
            let address = address.clone();
            let total_solutions = total_proofs.clone();
            let puzzle = puzzle.clone();
            thread::spawn(move || {
                core_affinity::set_for_current(core_affinity::CoreId { id: i });
                'outer: loop {
                    let current_proof_target = current_proof_target.clone();
                    let epoch_hash = epoch_hash.clone();
                    let address = address.clone();
                    let puzzle = puzzle.clone();
                    let total_solutions_ = total_solutions.clone();
                    if let (solution, proof_difficulty) = loop {
                        if epoch_number != current_epoch.load(Ordering::SeqCst) {
                            debug!(
                                "Terminating stale work: current {} latest {}",
                                epoch_number,
                                current_epoch.load(Ordering::SeqCst)
                            );
                            break 'outer;
                        }
                        debug!(
                            "Proving epoch {} with difficulty {}",
                            epoch_number,
                            current_proof_target.load(Ordering::SeqCst)
                        );
                        let counter = thread_rng().next_u64();
                        let res = Prover::prove(
                            &puzzle,
                            epoch_hash,
                            address,
                            counter,
                            Option::from(current_proof_target.load(Ordering::SeqCst)),
                        );
                        if res.is_ok() {
                            break res.unwrap();
                        }
                        total_solutions_.fetch_add(1, Ordering::SeqCst);
                    } {
                        if epoch_number != current_epoch.load(Ordering::SeqCst) {
                            debug!(
                                "Terminating stale work: current {} latest {}",
                                epoch_number,
                                current_epoch.load(Ordering::SeqCst)
                            );
                            break;
                        }

                        info!(
                            "Solution found for epoch {} with difficulty {}",
                            epoch_number, proof_difficulty
                        );

                        // Send a `PoolResponse` to the operator.
                        let message = Message::UnconfirmedSolution(UnconfirmedSolution {
                            solution_id: solution.id(),
                            solution: Data::Object(solution),
                        });
                        if let Err(error) = block_on(client.sender().send(message)) {
                            error!("Failed to send PoolResponse: {}", error);
                        }
                        total_solutions.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    }
}
