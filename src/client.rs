use std::{sync::Arc, time::Duration};

use futures_util::sink::SinkExt;
use snarkvm::dpc::{testnet2::Testnet2, Address};
use tokio::{
    net::TcpStream,
    sync::{
        mpsc,
        mpsc::{Receiver, Sender},
        Mutex,
    },
    task,
    time::{sleep, timeout},
};
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;
use tracing::{debug, error, info, warn};

use crate::{message::ProverMessage, prover::ProverEvent};

pub struct Client {
    address: Address<Testnet2>,
    server: String,
    sender: Arc<Sender<ProverMessage>>,
    receiver: Arc<Mutex<Receiver<ProverMessage>>>,
}

impl Client {
    pub fn init(address: Address<Testnet2>, server: String) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(1024);
        Arc::new(Self {
            address,
            server,
            sender: Arc::new(sender),
            receiver: Arc::new(Mutex::new(receiver)),
        })
    }

    pub fn sender(&self) -> Arc<Sender<ProverMessage>> {
        self.sender.clone()
    }

    pub fn receiver(&self) -> Arc<Mutex<Receiver<ProverMessage>>> {
        self.receiver.clone()
    }
}

pub fn start(prover_sender: Arc<Sender<ProverEvent>>, client: Arc<Client>) {
    task::spawn(async move {
        let receiver = client.receiver();
        loop {
            info!("Connecting to server...");
            match timeout(Duration::from_secs(5), TcpStream::connect(&client.server)).await {
                Ok(socket) => match socket {
                    Ok(socket) => {
                        info!("Connected to {}", client.server);
                        let mut framed = Framed::new(socket, ProverMessage::Canary);
                        let authorization =
                            ProverMessage::Authorize(client.address, String::new(), *ProverMessage::version());
                        if let Err(e) = framed.send(authorization).await {
                            error!("Error sending authorization: {}", e);
                        } else {
                            debug!("Sent authorization");
                        }
                        let receiver = &mut *receiver.lock().await;
                        while receiver.try_recv().is_ok() {}
                        loop {
                            tokio::select! {
                                Some(message) = receiver.recv() => {
                                    // let message = message.clone();
                                    let name = message.name();
                                    debug!("Sending {} to server", name);
                                    if let Err(e) = framed.send(message).await {
                                        error!("Error sending {}: {:?}", name, e);
                                    }
                                }
                                result = framed.next() => match result {
                                    Some(Ok(message)) => {
                                        debug!("Received {} from server", message.name());
                                        match message {
                                            ProverMessage::AuthorizeResult(result, message) => {
                                                if result {
                                                    debug!("Authorized");
                                                } else if let Some(message) = message {
                                                    error!("Authorization failed: {}", message);
                                                    sleep(Duration::from_secs(5)).await;
                                                    break;
                                                } else {
                                                    error!("Authorization failed");
                                                    sleep(Duration::from_secs(5)).await;
                                                    break;
                                                }
                                            }
                                            ProverMessage::Notify(block_template, share_difficulty) => {
                                                if let Err(e) = prover_sender.send(ProverEvent::NewWork(share_difficulty, block_template)).await {
                                                    error!("Error sending work to prover: {}", e);
                                                } else {
                                                    debug!("Sent work to prover");
                                                }
                                            }
                                            ProverMessage::SubmitResult(success, message) => {
                                                if let Err(e) = prover_sender.send(ProverEvent::Result(success, message)).await {
                                                    error!("Error sending share result to prover: {}", e);
                                                } else {
                                                    debug!("Sent share result to prover");
                                                }
                                            }
                                            _ => {
                                                debug!("Unhandled message: {}", message.name());
                                            }
                                        }
                                    }
                                    Some(Err(e)) => {
                                        warn!("Failed to read the message: {:?}", e);
                                    }
                                    None => {
                                        error!("Disconnected from server");
                                        sleep(Duration::from_secs(5)).await;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to connect to operator: {}", e);
                        sleep(Duration::from_secs(5)).await;
                    }
                },
                Err(_) => {
                    error!("Failed to connect to operator: Timed out");
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
}
