use crate::{utils, BoxcarMessage, RpcRequest, WireMessage};
use anyhow::bail;
use deadqueue::unlimited::Queue;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify, RwLock};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::instrument;

type Inbox = Arc<RwLock<BTreeMap<u16, Arc<Queue<BoxcarMessage>>>>>;

#[instrument]
async fn inbox_handler(
    mut rx: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    inbox: Inbox,
) {
    while let Some(message) = rx.next().await {
        tracing::trace!("received message from websocket");

        if let Ok(raw_message) = message {
            if let Message::Binary(raw) = raw_message {
                let message: WireMessage = utils::decode(&raw[..]).unwrap();

                tracing::trace!("waiting for inbox write guard");
                let queue_guard = inbox.write().await;
                tracing::trace!("acquired inbox write guard");

                if let Some(queue) = queue_guard.get(&message.c_slot) {
                    let msg = utils::decode(&message.inner[..]).unwrap();
                    tracing::trace!(c_slot = &message.c_slot, "decoded message. {:?}", &msg);
                    queue.push(msg);
                } else {
                    tracing::warn!(
                        c_slot = &message.c_slot,
                        "c_slot is not present. it might have been dropped. ignoring packet"
                    );
                }

                // explicitly drop the guard. it seems that it won't be implicitly dropped at
                // the end of the code path
                drop(queue_guard);
                tracing::trace!("dropped inbox write guard");
            } else {
                tracing::warn!("received unexpected websocket data type");
            }
        } else {
            tracing::warn!("unable to open message");
        }
    }
}

#[instrument]
async fn outbox_handler(
    mut ws_tx: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    mut rx: mpsc::Receiver<(WireMessage, Option<Arc<Notify>>)>,
) {
    while let Some(message) = rx.recv().await {
        let raw = utils::encode(&message.0).unwrap();
        match ws_tx
            .send(tokio_tungstenite::tungstenite::Message::Binary(raw))
            .await
        {
            Ok(_) => tracing::trace!("successfully flushed message to socket"),
            Err(err) => tracing::error!("unable to flush message to socket. {:?}", err),
        };
    }
}

#[derive(Clone, Debug)]
pub struct Client {
    /// c_slots are used for request-response requests
    outbox: mpsc::Sender<(WireMessage, Option<Arc<Notify>>)>,
    inbox: Inbox,
    slot_map: Arc<RwLock<HashMap<u16, u16>>>,
    handles: Arc<Vec<JoinHandle<()>>>,
}
impl Client {
    #[instrument]
    pub async fn new(addr: &str) -> anyhow::Result<Client> {
        tracing::info!("starting client");
        let (stream, _) = tokio_tungstenite::connect_async(addr).await?;

        let (tx, rx) = stream.split();
        let (outbox, outbox_rx) = mpsc::channel(1);

        let inbox: Inbox = Arc::new(Default::default());
        let slot_map = Arc::new(Default::default());

        let task_inbox = inbox.clone();
        // TODO can this be replaced by future_utils::pin_mut ?
        let mut handles = Vec::new();
        handles.push(tokio::task::spawn(async move {
            inbox_handler(rx, task_inbox).await
        }));
        handles.push(tokio::task::spawn(async move {
            outbox_handler(tx, outbox_rx).await
        }));

        tracing::info!("finished setting up client");

        Ok(Client {
            outbox,
            inbox,
            slot_map,
            handles: Arc::new(handles),
        })
    }

    #[instrument]
    async fn allocate_slot(&self) -> u16 {
        let mut depth = 0;

        loop {
            let slot: u16 = rand::thread_rng().gen();
            tracing::trace!(c_slot = slot, "accusing write handle for inbox");
            let mut slots = self.inbox.write().await;
            if let std::collections::btree_map::Entry::Vacant(e) = slots.entry(slot) {
                tracing::trace!(c_slot = slot, "slot {} is un-allocated, using", slot);
                e.insert(Arc::new(Queue::new()));

                drop(slots);
                tracing::trace!(c_slot = slot, "dropped write handle for inbox");

                return slot;
            }
            if depth > 1024 {
                // TODO handle this better so it's not just a panic
                panic!("oh no")
            }
            depth += 1;
        }
    }

    /// Send a BoxcarMessage, returning the expected c_slot where the response (might) be
    #[instrument]
    async fn send(&self, message: BoxcarMessage) -> anyhow::Result<u16> {
        // allocate a slot, which is where we will get the response
        let c_slot = self.allocate_slot().await;
        let message = WireMessage {
            c_slot,
            inner: utils::encode(message)?,
        };

        self.outbox.send((message, None)).await?;

        tracing::trace!("request sent. assigned slot {}", c_slot);

        Ok(c_slot)
    }

    #[instrument]
    pub async fn call(&self, inner: RpcRequest) -> anyhow::Result<u16> {
        tracing::debug!("sending {:?} for execution", &inner);
        let c_slot = self.send(BoxcarMessage::RpcReq(inner)).await?;

        tracing::trace!("acquiring write lock on inbox");
        let inbox_handle = self.inbox.read().await;
        tracing::trace!("acquired on write lock on inbox");

        let queue_slot = inbox_handle.get(&c_slot);
        if queue_slot.is_none() {
            bail!("slot does not exist")
        }

        let queue = queue_slot.unwrap().clone();
        drop(inbox_handle);
        tracing::trace!("dropped write lock on inbox");

        match queue.pop().await {
            BoxcarMessage::RpcReqSlot(s_slot) => {
                self.slot_map.write().await.insert(s_slot, c_slot);
                tracing::trace!(
                    s_slot = s_slot,
                    c_slot = c_slot,
                    "recording s_slot => c_slot mapping"
                );
                Ok(s_slot)
            }
            BoxcarMessage::ServerError(err) => bail!("server returned error. {}", err),
            _ => bail!("unexpected server response"),
        }
    }

    /// Close the client.
    ///
    /// 1. Un-subscribe from all slots
    /// 2. Notify the server we're shutting down
    /// 3. Shutdown
    #[instrument]
    pub async fn close(self) {
        let slots = self.slot_map.read().await;
        tracing::trace!("client is tracking {} slots before close", &slots.len());

        let message = BoxcarMessage::UnSub(slots.keys().cloned().collect::<Vec<u16>>());

        match self.send(message).await {
            Ok(_) => tracing::trace!("successfully sent unsubscribe command"),
            Err(err) => tracing::error!("unable to unsubscribe from server. {:?}", err),
        };

        // TODO should we shutdown self.outbox?

        for handle in self.handles.iter() {
            handle.abort();
        }
    }

    /// Return the expected c_slot given a (possibly subscribed) s_slot
    #[instrument]
    async fn get_c_slot(&self, s_slot: u16) -> Option<u16> {
        tracing::trace!(s_slot = s_slot, "attempting to get read handle on slot_map");
        let handle = self.slot_map.read().await;
        tracing::trace!(s_slot = s_slot, "acquired read handle on slot_map");

        let val = handle.get(&s_slot).copied();
        drop(handle);
        tracing::trace!(s_slot = s_slot, "dropped read handle on slot_map");

        val
    }

    #[instrument]
    pub async fn try_recv(&self, s_slot: u16) -> anyhow::Result<Option<BoxcarMessage>> {
        if let Some(slot) = self.get_c_slot(s_slot).await {
            let handle = self.inbox.read().await;
            if let Some(queue) = handle.get(&slot) {
                // // clone the queue, so we can drop the read handle.
                // // if it's not dropped, then this can block everything else.
                let q = queue.clone();
                // drop(handle);
                // TODO I don't think we need ^^ ?
                return Ok(q.try_pop());
            }
        }

        bail!("s_slot not subscribed to")
    }

    #[instrument]
    pub async fn recv(&self, s_slot: u16) -> anyhow::Result<BoxcarMessage> {
        if let Some(slot) = self.get_c_slot(s_slot).await {
            let handle = self.inbox.read().await;
            if let Some(queue) = handle.get(&slot) {
                // clone the queue, so we can drop the read handle.
                // if it's not dropped, then this can block everything else.
                let q = queue.clone();
                drop(handle);

                return Ok(q.pop().await);
            }
        }

        bail!("s_slot not subscribed to")
    }

    #[instrument]
    pub async fn subscribe(&self, s_slot: u16) -> anyhow::Result<()> {
        todo!()
    }

    #[instrument]
    pub async fn unsubscribe(&self, s_slot: u16) -> anyhow::Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use crate::client::Client;
    use crate::server::Server;
    use crate::{BoxcarExecutor, BoxcarMessage, BusWrapper, HandlerTrait, RpcResult};
    use async_trait::async_trait;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::time::sleep;

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
    async fn test_ping_pong() {
        let test_handler = TestHandler {};

        let mut executor = BoxcarExecutor::new();
        executor.add_handler(Box::new(test_handler)).await;

        let server = Server::new(TcpListener::bind("127.0.0.1:9931").await.unwrap(), executor);

        tokio::task::spawn(async move { server.serve().await });

        // give the server a minute to start up
        sleep(Duration::from_secs(1)).await;

        let ping_num = 2;

        let client = Client::new("ws://127.0.0.1:9931").await.unwrap();

        let call = client.send(BoxcarMessage::Ping(ping_num)).await;
        assert_eq!(call.is_ok(), true);

        let c_slot = call.unwrap();

        let queue_handle = client.inbox.read().await;
        let queue = queue_handle.get(&c_slot);
        assert_eq!(queue.is_some(), true);

        let queue = queue.unwrap().clone();
        drop(queue_handle);

        sleep(Duration::from_secs(1)).await;

        tracing::info!("waiting for queue to have a message");

        let msg = queue.pop().await;
        tracing::info!("{:?}", &msg);

        if let BoxcarMessage::Pong(num) = msg {
            assert_eq!(ping_num, num);
        } else {
            panic!("unexpected message returned");
        }
    }
}
