// TODO Implement RPC Result expiration on number of rpcs completed. By default purge after (2*max concurrent tasks) stored RPCResults
use std::collections::{BTreeMap, HashMap};
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
use tokio::net::{TcpListener, TcpStream};
use async_trait::async_trait;
use std::error::Error;
use anyhow::bail;

use deadqueue::limited::Queue;


pub trait BoxcarMessageTrait {
    fn encode(&self, buf: &mut Vec<u8>) -> Result<(), Box<dyn Error>>;
    fn decode(&self, buf: Vec<u8>) -> Result<(), Box<dyn Error>>;
}

#[async_trait]
pub trait HandlerTrait {
    async fn call(&self, method: &str, arguments: Vec<u8>) -> RpcResult;
    fn contains(&self, _: &str) -> bool;
    fn package(&self) -> &str;
}

pub type Handler = Box<dyn HandlerTrait + Sync + Send + 'static>;
pub type Callback = Box<dyn Fn(&str) -> bool + Sync + Send>;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
/// TODO Make this into something better
pub struct WireMessage {
    pub init: bool,
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
        tokio::spawn(async move {
            // handle each message in the mpsc channel
            while let Some(wire_message) = tx.recv().await {
                tracing::trace!("outgoing message: {:?}", &wire_message);
                // send the message out over the socket
                let raw =  bincode::serialize(&wire_message).unwrap();
                tracing::debug!("sending: {:?}", &raw);
                match ws_tx.send(tokio_tungstenite::tungstenite::Message::Binary(raw)).await {
                    Ok(_) => tracing::trace!("successfully flushed message to socket"),
                    Err(err) => tracing::error!("unable to flush message to socket. {:?}", err),
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
                                    Ok(_) => tracing::trace!("successfully flushed wire message to consumer queue"),
                                    Err(_) => tracing::error!("unable to flush message to consumer queue")
                                }
                            }
                            Message::Text(_) | Message::Pong(_) | Message::Ping(_) |Message::Close(_) => {
                                tracing::warn!("got message with non-binary data. ignoring");
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

        self.outbox.send(WireMessage { init: false, c_slot, subscribe, inner  }).await?;

        Ok(c_slot)
    }

    pub async fn send_initial(&self, c_slot: u16) -> Result<(), mpsc::error::SendError<WireMessage>> {

        self.outbox.send(WireMessage {
            init: true,
            c_slot,
            subscribe: false,
            inner: vec![]
        }).await?;

        // wait for the message to be flushed before continuing
        self.flush.notified().await;

        Ok(())
    }

    #[instrument]
    /// Receive an incoming message
    pub async fn recv(&self) -> WireMessage {
        self.rx.pop().await
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BoxcarMessage {
    /// Initiate an RPC
    RpcReq(RpcRequest),
    /// RPC Status
    RpcRslt(RpcResult),

    ServerError(String)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RpcRequest {
    pub method: String,
    pub body: Vec<u8>,
    pub delay: bool,
}

/// enum to represent the state of an RPC
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RpcResult {
    None,
    Ok(Vec<u8>),
    Err(Vec<u8>)
}

/// Represents an executing RPC Job
#[derive(Clone, Debug)]
pub struct RPCTask {
    slot: u16,
    request: RpcRequest,
    result: RpcResult
}

#[derive(Clone)]
pub struct BoxcarExecutor {
    /// List of all allocated slots
    slots: Arc<RwLock<Vec<u16>>>,
    /// Map that maps a Transport to all slots it is listening to
    subscriber_map: HashMap<Transport, Vec<u16>>,
    handlers: Arc<RwLock<Vec<Arc<Handler>>>>,
    tasks: Arc<RwLock<BTreeMap<u16, Arc<RwLock<RPCTask>>>>>
}
impl BoxcarExecutor {
    /// Add
    pub async fn add_handler(&mut self, handle: Handler) {
        self.handlers.write().await.push(Arc::new(handle))
    }

    /// Return the number of registered handlers
    pub async fn num_handlers(&self) -> usize {
        self.handlers.read().await.len()
    }

    pub async fn get_rpc(&self, slot: u16) -> Option<RPCTask> {
        match self.tasks.read().await.get(&slot) {
            None => None,
            Some(task) => Some(task.read().await.clone())
        }
    }

    async fn assign_slot(&mut self) -> u16 {
        let mut depth = 0;

        loop {
            // TODO Make this more efficient, as I think this is creating a new RNG each loop
            let slot: u16 = rand::thread_rng().gen();
            let mut task_handle = self.slots.write().await;

            if !task_handle.contains(&slot) {
                task_handle.push(slot);
                return slot;
            }
            if depth > 1024 {
                panic!("oh no")
            }
            depth += 1;
        }
    }

    pub async fn execute_task(&mut self, request: RpcRequest) -> anyhow::Result<u16> {
        let slot = self.assign_slot().await;
        let task = RPCTask {
            slot,
            request,
            result: RpcResult::None
        };
        let delay = task.request.delay;

        // search through registered handles to find the one that contains the requested method
        let handle = self
            .handlers.read().await;
        let handle = handle
            .iter()
            .find(|v| v.contains(task.request.method.as_str()));
        if handle.is_none() {
            bail!("no such handler");
        }


        // build task arc, and insert it into our map
        let task_ref = Arc::new(RwLock::new(task));
        self.tasks.write().await.insert(slot, task_ref.clone());

        let handler = handle.unwrap().clone();

        let closure = async move {
            let task_ref = task_ref.clone();

            // pull out the operation information, as we don't want to move stuff into our handler
            let task = task_ref.read().await;
            let slot = task.slot;

            // // run the handler
            tracing::debug!("---- entering task handler ----");
            let result = handler.call(task.request.method.as_str(), task.request.body.clone()).await;
            tracing::debug!("---- leaving  task handler ----");
            tracing::debug!("task slot {} returned {:?}", &slot, &result);

            // why is there a drop here? I don't know, but if you remove it- you don't save `result`
            drop(task);

            // take the result of the handler and write it into the RPCTask
            task_ref.write().await.result = result;
        };

        if delay {
            tracing::trace!("spawning task on a blocking thread");
            tokio::task::spawn_blocking(move || closure);
        } else {
            tracing::trace!("spawning task in a non-blocking fashion");
            tokio::task::spawn(closure);
        }

        Ok(slot)
    }

}

pub struct Server {
    bind_addr: String,
    executor: BoxcarExecutor,
}
impl Server {
    pub async fn serve(&self) -> anyhow::Result<()> {
        let try_socket = TcpListener::bind(&self.bind_addr).await;
        let listener = try_socket.expect("Failed to bind");
        tracing::info!("listening on: {}", &self.bind_addr);

        while let Ok((stream, addr)) = listener.accept().await {
            tracing::trace!(address = addr.to_string().as_str(), "accepted connection. spawning connection_handler");
            tokio::spawn(connection_handler(stream, addr, self.executor.clone()));
        }

        Ok(())
    }
}

pub async fn connection_handler(stream: TcpStream, addr: SocketAddr, mut executor: BoxcarExecutor) {
    let transport = Transport::server(stream, addr.clone()).await.unwrap();
    let addr = addr.to_string();

    // before we do anything, send a message to the client with a new c_slot
    let initial_slot = transport.allocate_slot().await;
    tracing::debug!(client = addr.as_str(), "allocating slot {} for initial communications", initial_slot);
    match transport.send_initial(initial_slot).await {
        Ok(_) => tracing::trace!(client = addr.as_str(), "successfully flushed initial packet"),
        Err(err) => {
            tracing::error!(client = addr.as_str(), "unable to send initial packet to client. {:?}", err)
        }
    }

    while let message = transport.recv().await {
        let boxcar_message: BoxcarMessage = bincode::deserialize(&*message.inner).unwrap();
        let response = match boxcar_message {
            BoxcarMessage::RpcReq(req) => {
                match executor.execute_task(req).await {
                    Ok(slot) => {
                        tracing::debug!(address = addr.as_str(), slot = slot, "successfully scheduled rpc");
                        match executor.get_rpc(slot).await {
                            None => panic!("just spawned a task, but executor does not contain the task info"),
                            Some(inner) => BoxcarMessage::RpcRslt(inner.result)
                        }

                    }
                    Err(error) => BoxcarMessage::ServerError(format!("unable to execute rpc. {:?}", error))
                }

            }
            BoxcarMessage::RpcRslt(_) => BoxcarMessage::ServerError("server got unexpected message".to_string()),
            _ => todo!()
        };

        match transport.send(Some(message.c_slot), bincode::serialize(&response).unwrap(), false).await {
            Ok(slot) => tracing::trace!(client = addr.as_str(), "successfully sent response to c_slot {}", slot),
            Err(err) => tracing::error!(client = addr.as_str(), "unable to send response. error: {:?}", err)
        }

    };
}

#[derive(Clone, Debug)]
pub struct Session {
    c_slot: u16,
    transport: Transport,
    inbox: Vec<BoxcarMessage>,
    notify: Arc<Notify>
}
impl Session {
    pub fn eq(&self, slot: u16) -> bool {
        self.c_slot == slot
    }

    pub async fn call(&self, req: RpcRequest) -> anyhow::Result<()> {
        let message = BoxcarMessage::RpcReq(req);
        let _q = self.transport.send(Some(self.c_slot), bincode::serialize(&message)?, false).await;

        Ok(())
    }

    pub async fn deliver(&mut self, _message: BoxcarMessage) {

    }
}

#[derive(Clone, Debug)]
pub struct Client {
    sessions: Arc<RwLock<Vec<Session>>>,
    transport: Transport
}
impl Client {
    pub async fn new(addr: &str) -> Self {
        let sessions: Arc<RwLock<Vec<Session>>> = Arc::new(Default::default());
        let transport = Transport::client(addr).await.unwrap();

        let trx = transport.clone();
        let sesh = sessions.clone();
        tokio::spawn(async move {
            while let message = trx.recv().await {
                let mut session_handle = sesh.write().await;
                let slot = message.c_slot;

                // locate the proper session to route the message to
                let handle = session_handle.iter_mut().find(|v| v.eq(slot));
                if handle.is_none() {
                    // TODO words
                    tracing::warn!("server sent message with unknown c_slot ");
                    continue;
                }

                let decoded = bincode::deserialize(&message.inner);
                let inner = handle.unwrap();
                let msg = decoded.unwrap();
                tracing::trace!(slot = slot, "delivering message to session inbox. {:?}", &msg);
                inner.inbox.push(msg);
            }
        });

        Self {
            sessions,
            transport
        }
    }

    pub async fn session(&self) -> Session {
        Session {
            c_slot: self.transport.allocate_slot().await,
            transport: self.transport.clone(),
            inbox: vec![],
            notify: Arc::new(Default::default())
        }
    }
}




#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;
    use async_trait::async_trait;
    use tokio::time::sleep;
    use crate::{BoxcarExecutor, Client, HandlerTrait, RpcRequest, RpcResult, Server};

    struct TestHandler { }
    #[async_trait]
    impl HandlerTrait for TestHandler {
        async fn call(&self, _method: &str, _arguments: Vec<u8>) -> RpcResult {
            sleep(Duration::from_secs(1)).await;
            RpcResult::Ok(vec![])
        }

        fn contains(&self, _: &str) -> bool {
            true
        }

        fn package(&self) -> &str {
            todo!()
        }
    }

    // #[tokio::test]
    // async fn test_executor() {
    //     let test_handler = TestHandler {};
    //
    //     let mut executor = BoxcarExecutor {
    //         slots: Arc::new(Default::default()),
    //         subscriber_map: Default::default(),
    //         handlers: Default::default(),
    //         tasks: Default::default()
    //     };
    //     executor.add_handler(Box::new(test_handler)).await;
    //
    //     assert_eq!(1, executor.num_handlers().await)
    //
    // }
    //
    // #[tokio::test]
    // async fn test_task_execution() {
    //     tracing_subscriber::fmt::init();
    //
    //     let test_handler = TestHandler {};
    //
    //     let mut executor = BoxcarExecutor {
    //         slots: Arc::new(Default::default()),
    //         subscriber_map: Default::default(),
    //         handlers: Default::default(),
    //         tasks: Default::default()
    //     };
    //     executor.add_handler(Box::new(test_handler)).await;
    //
    //     let task = RpcRequest {
    //         method: "foo".to_string(),
    //         body: vec![],
    //         delay: false
    //     };
    //     let req = executor.execute_task(task).await;
    //     assert_eq!(req.is_ok(), true);
    //
    //     sleep(Duration::from_secs(2)).await;
    //
    //     tracing::info!("tasks: {:?}", executor.tasks);
    //
    //     let result_handle = executor.tasks.read().await;
    //     let result = result_handle.get(&req.unwrap()).unwrap().read().await;
    //     match &result.result {
    //         RpcResult::None | RpcResult::Err(_) => assert_eq!(false, true),
    //         RpcResult::Ok(inner) => {
    //             let v: Vec<u8> = vec![];
    //             assert_eq!(*inner, v);
    //         },
    //     }
    // }

    #[tokio::test]
    async fn test_initial_packet() {
        tracing_subscriber::fmt::init();
        // create a server
        let server = Server {
            bind_addr: "127.0.0.1:9002".to_string(),
            executor: BoxcarExecutor { slots: Arc::new(Default::default()), subscriber_map: Default::default(), handlers: Arc::new(Default::default()), tasks: Arc::new(Default::default()) }
        };
        // spawn server
        tokio::task::spawn(async move {
            let _ = server.serve().await;
        });

        // give tokio a second to bring up the server
        sleep(Duration::from_secs(1)).await;

        // create a client
        let client = Client::new("ws://127.0.0.1:9002").await;
        // establish a session

        sleep(Duration::from_secs(1)).await
    }

}