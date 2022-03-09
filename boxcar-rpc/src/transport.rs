use std::net::SocketAddr;
use std::sync::Arc;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use tracing::instrument;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Notify};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use serde::{Serialize, Deserialize};

use deadqueue::limited::Queue;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
/// TODO Make this into something better
pub struct WireMessage {
    pub c_slot: u16,
    pub subscribe: bool,
    pub inner: Vec<u8>
}

#[derive(Debug, Clone)]
/// Ideally Vec<u8> could become T, but fuck getting the lifetimes to work
pub struct Transport {
    /// Outgoing messages
    outbox: mpsc::Sender<WireMessage>,
    /// Incomming Messages
    rx: Arc<Queue<WireMessage>>,

    /// Map of known c_slots
    /// TODO Make sure on drop of a session, the c_slot is freed
    c_slots: Arc<RwLock<Vec<u16>>>,

    flush: Arc<Notify>
}
impl Transport {
    pub async fn client<R: IntoClientRequest + Send + Unpin>(
        req: R
    ) -> Result<Transport, tokio_tungstenite::tungstenite::Error> {
        let s = tokio_tungstenite::connect_async(req).await?;
        Transport::build(
            s.0,
            SocketAddr::new("127.0.0.1".parse().unwrap(), 0),
        ).await
    }

    /// Build a Server endpoint
    pub async fn server<S: AsyncRead + AsyncWrite + Send + Unpin + 'static>(
        stream: S,
        addr: SocketAddr,
    ) -> Result<Transport, tokio_tungstenite::tungstenite::Error> {
        Transport::build(
            tokio_tungstenite::accept_async(stream).await?,
            addr
        ).await
    }

    /// Build the Endpoint. Establish channels and tasks to handle messages
    async fn build<W: AsyncRead + AsyncWrite + Unpin + Send + 'static>(ws: WebSocketStream<W>, addr: SocketAddr) -> Result<Self, tokio_tungstenite::tungstenite::Error> {
        let addr = addr.to_string();
        tracing::debug!(client = addr.as_str(), "building transport for client");
        let (mut ws_tx, mut ws_rx) = ws.split();

        // create incoming
        let (outbox, mut tx): (
            tokio::sync::mpsc::Sender<WireMessage>,
            tokio::sync::mpsc::Receiver<WireMessage>,
        ) = mpsc::channel(16);

        let flush = Arc::new(Notify::new());

        // Worker to handle outgoing messages
        let worker_flush = flush.clone();
        let addr2 = addr.clone();
        tokio::spawn(async move {
            // handle each message in the mpsc channel
            while let Some(wire_message) = tx.recv().await {
                tracing::trace!(client = addr2.as_str(), "outgoing message: {:?}", &wire_message);
                // send the message out over the socket
                let raw =  bincode::serialize(&wire_message).unwrap();
                tracing::debug!(client = addr2.as_str(), "successfully decoded message: {:?}", &raw);
                match ws_tx.send(tokio_tungstenite::tungstenite::Message::Binary(raw)).await {
                    Ok(_) => tracing::trace!(client = addr2.as_str(), "successfully flushed message to socket"),
                    Err(err) => tracing::error!(client = addr2.as_str(), "unable to flush message to socket. {:?}", err),
                };
                worker_flush.notify_one();
            }
        });

        // Worker to handle incoming messages
        let rx_queue: Arc<Queue<WireMessage>> = Arc::new(Queue::new(16));
        let rxa = rx_queue.clone();
        tokio::spawn(async move {
            while let Some(msg) = ws_rx.next().await {
                match msg {
                    Ok(packet) => {
                        tracing::trace!(client = addr.as_str(), "received message from client. {:?}", packet);
                        match packet {
                            Message::Binary(raw) => {
                                match rxa.try_push(bincode::deserialize(&raw[..]).unwrap()) {
                                    Ok(_) => tracing::trace!(client = addr.as_str(), "flushed wire message to consumer queue"),
                                    Err(_) => tracing::error!(client = addr.as_str(), "unable to flush message to consumer queue")
                                }
                            }
                            Message::Text(_) | Message::Pong(_) | Message::Ping(_) |Message::Close(_) => {
                                tracing::warn!(client = addr.as_str(), "got message with non-binary data. ignoring");
                                continue;
                            }
                        }
                    },
                    Err(_err) => {}
                }
            }
        });

        Ok(Transport {
            outbox,
            rx: rx_queue,
            c_slots: Arc::new(Default::default()),
            flush
        })
    }

    #[instrument]
    pub async fn allocate_slot(&self) -> u16 {
        let mut depth = 0;
        let mut slots = self.c_slots.write().await;

        loop {
            let slot: u16 = rand::thread_rng().gen();
            if !slots.contains(&slot) {
                // make sure the slot is recorded as in use
                slots.push(slot);
                return slot;
            }
            if depth > 1024 {
                // TODO handle this better so it's not just a panic
                panic!("oh no")
            }
            depth += 1;
        }
    }

    #[instrument]
    /// Place a message in the outbox, and return the c_slot
    pub async fn send(&self, c_slot: Option<u16>, inner: Vec<u8>, subscribe: bool) -> Result<u16, mpsc::error::SendError<WireMessage>> {
        let c_slot = match c_slot {
            None => self.allocate_slot().await,
            Some(slot) => slot
        };

        self.outbox.send(WireMessage { c_slot, subscribe, inner  }).await?;

        Ok(c_slot)
    }

    #[instrument]
    /// Receive an incoming message
    pub async fn recv(&self) -> WireMessage {
        self.rx.pop().await
    }
}
