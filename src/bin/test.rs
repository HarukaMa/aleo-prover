use aleo_prover::ProverMessage;
use futures_util::sink::SinkExt;
use snarkvm::dpc::{testnet2::Testnet2, BlockTemplate};
use snarkvm::utilities::FromBytes;
use std::fs::File;
use tokio::sync::oneshot;
use std::process::Command;
use std::time::Duration;
use tokio::sync::mpsc::channel;
use tokio::task;
use tokio::{net::TcpListener, time::sleep};
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;
use tracing::{error, info};

/// Usage: cargo run --bin test -- <notify_interval_in_millions>
#[tokio::main]
async fn main() {
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("unable to set global default subscriber");

    info!("starting fake server");
    let interval = std::env::args()
        .collect::<Vec<String>>()
        .get(1)
        .unwrap_or(&"15000".to_string())
        .parse::<u64>()
        .unwrap();
    info!("notify interval: {}ms", interval);
    start_fake_server(interval).await;

    let prover = Command::new("cargo")
        .arg("run")
        .arg("--bin")
        .arg("aleo-prover")
        .arg("--")
        .arg("-a")
        .arg("aleo1j9tthvexfs2232247jc8wencgslsnydhyx3vvr7vv7mruqeptgpq4ektaj")
        .arg("-p")
        .arg("127.0.0.1:9090")
        .arg("--debug")
        .status()
        .unwrap();
    info!("exit: {}", prover);

    std::future::pending::<()>().await;
}

async fn start_fake_server(interval: u64) {
    let mut templates = Vec::with_capacity(20);
    for i in 0..20 {
        let mut file = File::open(format!("templates/{}", 403591 + i)).unwrap();
        let template = BlockTemplate::<Testnet2>::read_le(&mut file).unwrap();
        templates.push(template);
    }
    info!("templates generated");

    let listener = TcpListener::bind(format!("0.0.0.0:{}", 9090)).await.unwrap();
    let (tx, mut rx) = channel(1);
    let (one_t, one_r) = oneshot::channel();
    task::spawn(async move {
        match listener.accept().await {
            Ok((stream, peer)) => {
                info!("new connection from prover: {}", peer);
                let mut frame = Framed::new(stream, ProverMessage::Canary);
                if let Some(Ok(ProverMessage::Authorize(_, _, _))) = frame.next().await {
                    one_t.send(()).unwrap();
                } else {
                    error!("authorize failed");
                }
                loop {
                    tokio::select! {
                        Some(msg) = rx.recv() => {
                            if let Err(_) = frame.send(msg).await {
                                error!("Failed to send message to prover");
                            }
                        }
                        result = frame.next() => match result {
                           Some(Ok(msg)) => {
                               match msg {
                                   ProverMessage::Submit(height, _, _) => {
                                        info!("received submit from prover: {}", height);
                                   }
                                   _ => {}
                               }
                           }
                           _ => {}
                       }
                    }
                }
            }
            Err(e) => {
                error!("Error accepting connection: {:?}", e);
            }
        }
    });

    task::spawn(async move {
        let templates = templates;
        one_r.await.unwrap();
        for i in 0..20 {
            let template = templates[i % 20].clone();
            info!("sending height: {}", template.block_height());
            let msg = ProverMessage::Notify(template, u64::MAX);
            let _ = tx.send(msg).await;
            sleep(Duration::from_millis(interval)).await;
        }
    });
}
