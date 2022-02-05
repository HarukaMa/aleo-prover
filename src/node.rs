use std::{collections::BTreeMap, sync::Arc, time::Duration};

use futures_util::sink::SinkExt;
use rand::{thread_rng, Rng};
use snarkos::{
    environment::Prover,
    helpers::{NodeType, State},
    Data,
    Message,
};
use snarkos_storage::BlockLocators;
use snarkvm::{
    dpc::{testnet2::Testnet2, Address, BlockHeader},
    traits::Network,
};
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

use crate::{prover::ProverEvent, Client};

pub struct Node {
    address: Address<Testnet2>,
    operator: String,
    router: Arc<Sender<SendMessage>>,
    receiver: Arc<Mutex<Receiver<SendMessage>>>,
}

#[derive(Debug)]
pub struct SendMessage {
    pub(crate) message: Message<Testnet2, Prover<Testnet2>>,
}

impl Node {
    pub fn init(address: Address<Testnet2>, operator: String) -> Arc<Self> {
        let (router_tx, router_rx) = mpsc::channel(1024);
        Arc::new(Self {
            address,
            operator,
            router: Arc::new(router_tx),
            receiver: Arc::new(Mutex::new(router_rx)),
        })
    }

    pub fn router(&self) -> Arc<Sender<SendMessage>> {
        self.router.clone()
    }

    pub fn receiver(&self) -> Arc<Mutex<Receiver<SendMessage>>> {
        self.receiver.clone()
    }
}

pub fn start(prover_router: Arc<Sender<ProverEvent>>, client: Arc<Client>) {
    task::spawn(async move {
        let receiver = client.receiver();
        loop {
            info!("Connecting to operator...");
            match timeout(Duration::from_secs(5), TcpStream::connect(&client.pool)).await {
                Ok(socket) => match socket {
                    Ok(socket) => {
                        info!("Connected to {}", client.pool);
                        let mut framed = Framed::new(socket, Message::<Testnet2, Prover<Testnet2>>::PeerRequest);
                        let challenge = Message::ChallengeRequest(
                            12,
                            Testnet2::ALEO_MAXIMUM_FORK_DEPTH,
                            NodeType::Prover,
                            State::Ready,
                            4132,
                            thread_rng().gen(),
                            0,
                        );
                        if let Err(e) = framed.send(challenge).await {
                            error!("Error sending challenge request: {}", e);
                        } else {
                            debug!("Sent challenge request");
                        }
                        let receiver = &mut *receiver.lock().await;
                        loop {
                            tokio::select! {
                                Some(message) = receiver.recv() => {
                                    let message = message.clone();
                                    debug!("Sending {} to operator", message.name());
                                    if let Err(e) = framed.send(message.clone()).await {
                                        error!("Error sending {}: {:?}", message.name(), e);
                                    }
                                }
                                result = framed.next() => match result {
                                    Some(Ok(message)) => {
                                        debug!("Received {} from operator", message.name());
                                        match message {
                                            Message::ChallengeRequest(..) => {
                                                let resp = Message::ChallengeResponse(Data::Object(Testnet2::genesis_block().header().clone()));
                                                if let Err(e) = framed.send(resp).await {
                                                    error!("Error sending challenge response: {:?}", e);
                                                } else {
                                                    debug!("Sent challenge response");
                                                }
                                            }
                                            Message::ChallengeResponse(..) => {
                                                let ping = Message::<Testnet2, Prover<Testnet2>>::Ping(
                                                    12,
                                                    Testnet2::ALEO_MAXIMUM_FORK_DEPTH,
                                                    NodeType::Prover,
                                                    State::Ready,
                                                    Testnet2::genesis_block().hash(),
                                                    Data::Object(Testnet2::genesis_block().header().clone()),
                                                );
                                                if let Err(e) = framed.send(ping).await {
                                                    error!("Error sending ping: {:?}", e);
                                                } else {
                                                    debug!("Sent ping");
                                                }
                                            }
                                            Message::Ping(..) => {
                                                let mut locators: BTreeMap<u32, (<Testnet2 as Network>::BlockHash, Option<BlockHeader<Testnet2>>)> = BTreeMap::new();
                                                locators.insert(0, (Testnet2::genesis_block().hash(), None));
                                                let resp = Message::<Testnet2, Prover<Testnet2>>::Pong(None, Data::Object(BlockLocators::<Testnet2>::from(locators).unwrap_or_default()));
                                                if let Err(e) = framed.send(resp).await {
                                                    error!("Error sending pong: {:?}", e);
                                                } else {
                                                    debug!("Sent pong");
                                                }
                                            }
                                            Message::Pong(..) => {
                                                let register = Message::<Testnet2, Prover<Testnet2>>::PoolRegister(client.address);
                                                if let Err(e) = framed.send(register).await {
                                                    error!("Error sending pool register: {:?}", e);
                                                } else {
                                                    debug!("Sent pool register");
                                                }
                                            }
                                            Message::PoolRequest(share_difficulty, block_template) => {
                                                if let Ok(block_template) = block_template.deserialize().await {
                                                    if let Err(e) = prover_router.send(ProverWork::new(share_difficulty, block_template)).await {
                                                        error!("Error sending work to prover: {:?}", e);
                                                    } else {
                                                        debug!("Sent work to prover");
                                                    }
                                                } else {
                                                    error!("Error deserializing block template");
                                                }
                                            }
                                            Message::UnconfirmedBlock(..) => {}
                                            _ => {
                                                debug!("Unhandled message: {}", message.name());
                                            }
                                        }
                                    }
                                    Some(Err(e)) => {
                                        warn!("Failed to read the message: {:?}", e);
                                    }
                                    None => {
                                        error!("Disconnected from operator");
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
