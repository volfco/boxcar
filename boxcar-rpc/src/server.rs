use crate::{utils, BoxcarExecutor, BoxcarMessage, RpcRequest, WireMessage};
use futures_util::{SinkExt, StreamExt};
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify, RwLock};
use tokio_tungstenite::tungstenite::Message;
use tracing::instrument;

pub struct Server {
    listener: TcpListener,
    executor: BoxcarExecutor,
}
impl Server {
    pub fn new(listener: TcpListener, executor: BoxcarExecutor) -> Self {
        Server { listener, executor }
    }
    #[instrument]
    pub async fn serve(&self) {
        while let Ok((stream, addr)) = self.listener.accept().await {
            tracing::trace!(
                address = addr.to_string().as_str(),
                "accepted connection. spawning connection_handler"
            );
            tokio::spawn(connection_handler(stream, addr, self.executor.clone()));
        }
    }
}
impl Debug for Server {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.listener)
    }
}

///
/// c_slot 0 is reserved for unsolicited communication
#[instrument]
async fn connection_handler(stream: TcpStream, addr: SocketAddr, executor: BoxcarExecutor) {
    let addr = addr.to_string();
    let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
    let (mut ws_tx, mut ws_rx) = ws.split();

    // list of s_slots this connection is subscribed to
    let subcribed: Arc<RwLock<BTreeMap<u16, u16>>> = Arc::new(RwLock::new(BTreeMap::new()));

    // channel to send outgoing messages to
    let (outbox_tx, mut outbox_rx): (
        tokio::sync::mpsc::Sender<(WireMessage, Option<Arc<Notify>>)>,
        tokio::sync::mpsc::Receiver<(WireMessage, Option<Arc<Notify>>)>,
    ) = mpsc::channel(1);

    let mut listener = executor.get_listener();
    let subscribed_handle = subcribed.clone();
    let outbox_handle = outbox_tx.clone();

    // TODO Don't encode the message per subscription. But do it once and then send it on
    tokio::spawn(async move {
        while let Ok(inner) = listener.recv().await {
            if let Some(c_slot) = subscribed_handle.read().await.get(&inner.0) {
                // tracing::debug!("client is subscribed to slot, relaying message");
                let inner = BoxcarMessage::RpcRslt(inner);
                outbox_handle
                    .send((
                        WireMessage {
                            c_slot: *c_slot,
                            inner: utils::encode(inner).unwrap(),
                        },
                        None,
                    ))
                    .await;
            }
        }
    });

    let caddr = addr.clone();
    tokio::spawn(async move {
        while let Some((message, notify)) = outbox_rx.recv().await {
            match utils::encode(message) {
                Ok(raw) => {
                    match ws_tx
                        .send(tokio_tungstenite::tungstenite::Message::Binary(raw))
                        .await
                    {
                        Ok(_) => {
                            tracing::trace!(
                                client = caddr.as_str(),
                                "successfully wrote message to socket"
                            )
                        }
                        Err(err) => tracing::error!(
                            client = caddr.as_str(),
                            "unable to flush message to socket due to error: {:?}",
                            err
                        ),
                    };
                    if let Some(notif) = notify {
                        notif.notify_one();
                    }
                }
                Err(err) => tracing::error!(
                    client = caddr.as_str(),
                    "unable to encode message. {:?}",
                    err
                ),
            }
        }
    });

    while let Some(message) = ws_rx.next().await {
        match message {
            Ok(inner) => {
                // spawn a task to handle the processing of the individual message
                let outbound_chan = outbox_tx.clone();
                let task_exec = executor.clone();
                let task_subcribed = subcribed.clone();
                let caddr = addr.clone();
                tokio::spawn(async move {
                    tracing::trace!(
                        client = caddr.as_str(),
                        "received message from client. {:?}",
                        &inner
                    );

                    if let Message::Binary(raw) = inner {
                        let result = message_handler(
                            utils::decode(&raw[..]).unwrap(),
                            task_exec,
                            task_subcribed,
                        )
                        .await;

                        outbound_chan.send((result, None)).await;
                    } else {
                        tracing::warn!(
                            client = caddr.as_str(),
                            "got message with non-binary data. ignoring"
                        );
                    }
                });
            }
            Err(error) => {
                tracing::error!(
                    client = addr.as_str(),
                    "unable to read message. {:?}",
                    error
                );
            }
        };
    }
}

#[instrument]
async fn message_handler(
    message: WireMessage,
    executor: BoxcarExecutor,
    subscribed: Arc<RwLock<BTreeMap<u16, u16>>>,
) -> WireMessage {
    tracing::trace!("handling {:?}", &message);

    let inner: BoxcarMessage = bincode::deserialize(&message.inner[..]).unwrap();
    tracing::trace!("request decoded into {:?}", &inner);

    let response = match inner {
        BoxcarMessage::RpcReq(req) => {
            let sub = req.subscribe;
            let rsp = handle_rpc_req(req, executor).await;
            if sub {
                if let BoxcarMessage::RpcReqSlot(s_slot) = rsp.clone() {
                    tracing::trace!("request subscribed, registering subscription");
                    subscribed.write().await.insert(s_slot, message.c_slot);
                }
            }

            rsp
        }
        BoxcarMessage::RpcRslt(_) => todo!(),
        BoxcarMessage::Sub(slots) => handle_sub(slots, subscribed.clone(), message.c_slot).await,
        BoxcarMessage::UnSub(slots) => handle_unsub(slots, subscribed.clone()).await,
        BoxcarMessage::Hangup => todo!(),
        BoxcarMessage::Ping(num) => handle_ping(num).await,
        BoxcarMessage::Pong(_) => {
            tracing::warn!("BoxcarMessage::Pong is unexpected. ignoring");
            BoxcarMessage::ServerError("unsupported message".to_string())
        }
        _ => todo!(),
    };

    tracing::trace!("responding {:?}", &response);

    WireMessage {
        c_slot: message.c_slot,
        inner: utils::encode(response).unwrap(),
    }
}

#[instrument]
async fn handle_sub(
    slots: Vec<u16>,
    subscribed: Arc<RwLock<BTreeMap<u16, u16>>>,
    c_slot: u16,
) -> BoxcarMessage {
    tracing::trace!("getting write handle on client subscriber map");
    let mut handle = subscribed.write().await;
    tracing::trace!("acquired write handle");

    let mut changed = false;
    for slot in slots {
        if handle.contains_key(&slot) {
            tracing::trace!(s_slot = slot, "slot is already mapped for client")
        } else {
            changed = true;
            handle.insert(slot, c_slot.clone());
            tracing::debug!(s_slot = slot, "subscribed client to slot");
        }
    }

    BoxcarMessage::SubOpFin(changed)
}

#[instrument]
async fn handle_unsub(
    slots: Vec<u16>,
    subscribed: Arc<RwLock<BTreeMap<u16, u16>>>,
) -> BoxcarMessage {
    tracing::trace!("getting write handle on client subscriber map");
    let mut handle = subscribed.write().await;
    tracing::trace!("acquired write handle");

    let mut changed = false;
    for slot in slots {
        if handle.contains_key(&slot) {
            changed = true;
            handle.remove(&slot);
            tracing::debug!(s_slot = slot, "unsubscribed client from slot")
        } else {
            tracing::trace!(s_slot = slot, "client not subscribed to slot");
        }
    }

    BoxcarMessage::SubOpFin(changed)
}

/// Respond to a ping with a pong
#[instrument]
async fn handle_ping(num: u8) -> BoxcarMessage {
    BoxcarMessage::Pong(num)
}

#[instrument]
async fn handle_rpc_req(req: RpcRequest, mut executor: BoxcarExecutor) -> BoxcarMessage {
    match executor.execute_task(req).await {
        Ok(s_slot) => BoxcarMessage::RpcReqSlot(s_slot),
        Err(err) => BoxcarMessage::ServerError(format!("unable to schedule request. {:?}", err)),
    }
}

#[cfg(test)]
mod tests {
    use crate::server::Server;
    use crate::{
        utils, BoxcarExecutor, BoxcarMessage, BusWrapper, HandlerTrait, RpcResult, WireMessage,
    };
    use async_trait::async_trait;
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::time::sleep;
    use tokio_tungstenite::tungstenite::Message;

    struct TestHandler {}
    #[async_trait]
    impl HandlerTrait for TestHandler {
        async fn call(&self, _method: &str, _arguments: Vec<u8>, _bus: BusWrapper) -> RpcResult {
            sleep(Duration::from_secs(1)).await;
            RpcResult::Ok(vec![])
        }

        fn contains(&self, _method: &str) -> bool {
            true
        }

        fn package(&self) -> &str {
            todo!()
        }
    }

    #[tokio::test]
    async fn test_basic_ping_pong() {
        let test_handler = TestHandler {};

        let mut executor = BoxcarExecutor::new();
        executor.add_handler(Box::new(test_handler)).await;

        let server = Server {
            listener: TcpListener::bind("127.0.0.1:9932").await.unwrap(),
            executor,
        };

        tokio::task::spawn(async move { server.serve().await });

        // give the server a minute to start up
        sleep(Duration::from_secs(1)).await;

        let (mut client, _) = tokio_tungstenite::connect_async("ws://127.0.0.1:9932")
            .await
            .unwrap();

        let ping_num = 2;

        let wire_message = WireMessage {
            c_slot: 0,
            inner: utils::encode(BoxcarMessage::Ping(ping_num)).unwrap(),
        };
        let raw_message = utils::encode(wire_message).unwrap();

        let send = client.send(Message::Binary(raw_message)).await;
        assert_eq!(send.is_ok(), true);

        sleep(Duration::from_secs(1)).await;

        let response = client.next().await;
        assert_eq!(response.is_some(), true);

        let inner = response.unwrap();
        assert_eq!(inner.is_ok(), true);

        let message = inner.unwrap();
        if let Message::Binary(raw) = message {
            let wire_message: WireMessage = utils::decode(&raw[..]).unwrap();
            let boxcar_message: BoxcarMessage = utils::decode(&wire_message.inner[..]).unwrap();

            if let BoxcarMessage::Pong(num) = boxcar_message {
                assert_eq!(ping_num, num);
            } else {
                panic!("unexpected message returned. {:?}", boxcar_message);
            }
        } else {
            panic!("incorrect message type")
        }
    }
}
