use crate::rcm::{Claim, ResourceError, ResourceManager};
use crate::{utils, BoxcarExecutor, BoxcarMessage, RpcRequest, WireMessage};
use futures_util::{SinkExt, StreamExt};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify, RwLock};
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, instrument, trace, warn};

pub struct Server {
    pub bind: SocketAddr,
    pub executor: BoxcarExecutor,
    pub resource_manager: ResourceManager,
}
impl Server {
    pub fn new() -> Self {
        Server {
            bind: "127.0.0.1:9930".parse().unwrap(),
            executor: BoxcarExecutor::new(),
            resource_manager: ResourceManager::new().permissive(true),
        }
    }
    pub fn bind(self, bind: SocketAddr) -> Self {
        Server { bind, ..self }
    }
    pub fn executor(self, executor: BoxcarExecutor) -> Self {
        Server { executor, ..self }
    }
    pub fn resource_manager(self, resource_manager: ResourceManager) -> Self {
        Server {
            resource_manager,
            ..self
        }
    }

    #[instrument]
    pub async fn serve(&self) -> anyhow::Result<()> {
        // TODO Implement a finite slot limit for connections, so a connection will occupy a specific slot
        while let Ok((stream, addr)) = TcpListener::bind(self.bind).await?.accept().await {
            trace!(
                address = addr.to_string().as_str(),
                "accepted connection. spawning connection_handler"
            );
            tokio::spawn(connection_handler(
                stream,
                addr,
                self.executor.clone(),
                self.resource_manager.clone(),
            ));
        }

        Ok(())
    }
}
impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}
impl Debug for Server {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.bind)
    }
}

type WireMessageChan = (WireMessage, Option<Arc<Notify>>);

///
/// c_slot 0 is reserved for unsolicited communication
#[instrument]
async fn connection_handler(
    stream: TcpStream,
    addr: SocketAddr,
    executor: BoxcarExecutor,
    resource_manager: ResourceManager,
) {
    let addr = addr.to_string();
    let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
    let (mut ws_tx, mut ws_rx) = ws.split();

    // list of s_slots this connection is subscribed to
    let subcribed: Arc<RwLock<BTreeMap<u16, u16>>> = Arc::new(RwLock::new(BTreeMap::new()));

    // channel to send outgoing messages to
    // TODO should this be only one? This could gum up everything if the flushing-to-client is
    //      slower than the rate messages are produced.
    let (outbox_tx, mut outbox_rx): (
        mpsc::Sender<WireMessageChan>,
        mpsc::Receiver<WireMessageChan>,
    ) = mpsc::channel(1);

    // async task to listen to the executor's message bus, and relay messages about tasks this
    // client has subscribed to.
    // TODO Don't encode the message per subscription. But do it once and then send it on
    let mut listener = executor.get_listener();
    let subscribed_handle = subcribed.clone();
    let outbox_handle = outbox_tx.clone();
    tokio::spawn(async move {
        while let Ok(inner) = listener.recv().await {
            // check to see if the message's s_slot is something we care about
            // TODO is there a more efficient way to do this where we don't need to get a read
            //      handle for each message?
            let s_slot = inner.0;
            if let Some(c_slot) = subscribed_handle.read().await.get(&s_slot) {
                // tracing::debug!("client is subscribed to slot, relaying message");
                // TODO add tracing here?
                if let Err(e) = outbox_handle
                    .send((
                        WireMessage {
                            c_slot: *c_slot,
                            inner: utils::encode(BoxcarMessage::RpcRslt(inner)).unwrap(),
                        },
                        None,
                    ))
                    .await
                {
                    error!(
                        s_slot = s_slot,
                        c_slot = c_slot,
                        "unable to flush message to outbox. {:?}",
                        e
                    );
                } else {
                    trace!(
                        s_slot = s_slot,
                        c_slot = c_slot,
                        "flushed message to outbox"
                    )
                }
            }
        }
    });

    // Outbox handler. Takes messages from the outbox channel and flushes them to the client via
    // the websocket... socket
    let caddr = addr.clone();
    tokio::spawn(async move {
        while let Some((message, notify)) = outbox_rx.recv().await {
            match utils::encode(message) {
                Ok(raw) => {
                    match ws_tx.send(Message::Binary(raw)).await {
                        Ok(_) => {
                            trace!(
                                client = caddr.as_str(),
                                "successfully wrote message to socket"
                            )
                        }
                        Err(err) => error!(
                            client = caddr.as_str(),
                            "unable to flush message to socket due to error: {:?}", err
                        ),
                    };
                    if let Some(notif) = notify {
                        notif.notify_one();
                    }
                }
                Err(err) => error!(
                    client = caddr.as_str(),
                    "unable to encode message. {:?}", err
                ),
            }
        }
    });

    // handle inbound messages
    while let Some(message) = ws_rx.next().await {
        match message {
            Ok(inner) => {
                // spawn a task to handle the processing of the individual message
                let outbound_chan = outbox_tx.clone();
                let task_exec = executor.clone();
                let task_subcribed = subcribed.clone();
                let caddr = addr.clone();
                let rcm = resource_manager.clone();
                tokio::spawn(async move {
                    trace!(
                        client = caddr.as_str(),
                        "received message from client. {:?}",
                        &inner
                    );

                    if let Message::Binary(raw) = inner {
                        let result = message_handler(
                            utils::decode(&raw[..]).unwrap(),
                            task_exec,
                            task_subcribed,
                            rcm,
                        )
                        .await;

                        if let Err(err) = outbound_chan.send((result, None)).await {
                            error!(
                                client = caddr.as_str(),
                                "unable to flush message. {:?}", err
                            )
                        }
                    } else {
                        warn!(
                            client = caddr.as_str(),
                            "got message with non-binary data. ignoring"
                        );
                    }
                });
            }
            Err(error) => {
                error!(
                    client = addr.as_str(),
                    "unable to read message. {:?}", error
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
    resource_manager: ResourceManager,
) -> WireMessage {
    trace!("handling {:?}", &message);

    // TODO handle this error and not just blindly unwrap()
    let inner: BoxcarMessage = bincode::deserialize(&message.inner[..]).unwrap();
    trace!("request decoded into {:?}", &inner);

    let response = match inner {
        BoxcarMessage::RpcReq(req) => {
            let sub = req.subscribe;

            let mut claims = vec![];
            let mut error = None;
            for resource in req.resources.iter() {
                trace!(
                    "request has is requesting resource allocations. {:?}",
                    &resource
                );
                match resource_manager.consume(resource.0, *resource.1) {
                    Ok(claim) => claims.push(claim),
                    Err(err) => {
                        error = Some(err);
                        break;
                    }
                }
            }

            if claims.len() != req.resources.len() {
                warn!("unable to satisfy all resource requests");
                if let Some(error) = error {
                    BoxcarMessage::ResourceError(error)
                } else {
                    BoxcarMessage::ResourceError(ResourceError::Unknown)
                }
            } else {
                let rsp = handle_rpc_req(req, executor, claims).await;
                if sub {
                    if let BoxcarMessage::RpcReqRslt(s_slot) = rsp.clone() {
                        trace!("request subscribed, registering subscription");
                        subscribed.write().await.insert(s_slot, message.c_slot);
                    }
                }

                rsp
            }
        }
        BoxcarMessage::RpcReqRslt(s_slot) => handle_rpc_req_result(s_slot, executor).await,
        BoxcarMessage::RpcRslt(_) => todo!(),
        BoxcarMessage::Sub(slots) => handle_sub(slots, subscribed.clone(), message.c_slot).await,
        BoxcarMessage::UnSub(slots) => handle_unsub(slots, subscribed.clone()).await,
        BoxcarMessage::Hangup => todo!(),
        BoxcarMessage::Ping(num) => handle_ping(num).await,
        BoxcarMessage::Pong(_) => {
            warn!("BoxcarMessage::Pong is unexpected. ignoring");
            BoxcarMessage::ServerError("unsupported message".to_string())
        }
        _ => todo!(),
    };

    trace!("responding {:?}", &response);

    WireMessage {
        c_slot: message.c_slot,
        inner: utils::encode(response).unwrap(),
    }
}

#[instrument]
async fn handle_rpc_req_result(s_slot: u16, executor: BoxcarExecutor) -> BoxcarMessage {
    let handle = executor.tasks.read().await;
    let task_handle = handle.get(&s_slot);
    if let Some(task) = task_handle {
        let inner = task.read().await;
        BoxcarMessage::RpcRslt((s_slot, inner.result.clone()))
    } else {
        BoxcarMessage::ServerError("no such s_slot".to_string())
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
        if let Entry::Vacant(e) = handle.entry(slot) {
            changed = true;
            e.insert(c_slot);
            tracing::debug!(s_slot = slot, "subscribed client to slot");
        } else {
            tracing::trace!(s_slot = slot, "slot is already mapped for client")
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
async fn handle_rpc_req(
    req: RpcRequest,
    mut executor: BoxcarExecutor,
    claims: Vec<Claim>,
) -> BoxcarMessage {
    match executor.execute_task(req, claims).await {
        Ok(s_slot) => BoxcarMessage::RpcReqRslt(s_slot),
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

        let server = Server::new()
            .bind("127.0.0.1:9932".parse().unwrap())
            .executor(executor);

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
