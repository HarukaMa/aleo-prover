use aleo_prover::ProverMessage;
use futures_util::sink::SinkExt;
use snarkvm::dpc::{testnet2::Testnet2, BlockTemplate};
use snarkvm::utilities::FromBytes;
use std::fs::File;
use std::process::Command;
use std::time::Duration;
use tokio::task;
use tokio::{net::TcpListener, time::sleep};
// use tokio_stream::StreamExt;
use tokio_util::codec::Framed;
use tracing::{error, info};

/// Usage: ./test <notify_interval_in_millions>
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
        .unwrap()
        .parse::<u64>()
        .unwrap();
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
    task::spawn(async move {
        let templates = templates;
        match listener.accept().await {
            Ok((stream, peer)) => {
                info!("new connection from prover: {}", peer);
                let mut frame = Framed::new(stream, ProverMessage::Canary);
                for i in 0..20 {
                    let template = templates[i % 20].clone();
                    info!("sending height: {}", template.block_height());
                    frame.send(ProverMessage::Notify(template, u64::MAX)).await.unwrap();
                    sleep(Duration::from_millis(interval)).await;
                }
            }
            Err(e) => {
                error!("Error accepting connection: {:?}", e);
            }
        }
    });
}
