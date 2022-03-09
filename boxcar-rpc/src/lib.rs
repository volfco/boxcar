pub mod executor;
pub mod transport;

pub use crate::transport::{WireMessage, Transport};

// TODO Implement RPC Result expiration on number of rpcs completed. By default purge after (2*max concurrent tasks) stored RPCResults
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::sync::RwLock;

use serde::{Serialize, Deserialize};
use tokio::net::{TcpListener, TcpStream};
use async_trait::async_trait;
use std::error::Error;
use deadqueue::limited::Queue;


use crate::executor::BoxcarExecutor;


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


#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BoxcarMessage {
    /// Initiate an RPC
    RpcReq(RpcRequest),
    /// RPC Status
    RpcRslt((u16, RpcResult)),

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

async fn rpc_callback(c_slot: u16, s_slot: u16, notify: Arc<Notify>, transport: Transport, executor: BoxcarExecutor) {
    notify.notified().await;
    let result = executor.get_rpc(s_slot).await;
    let message = BoxcarMessage::RpcRslt((s_slot, result.unwrap().result));
    tracing::debug!(c_slot = c_slot, s_slot = s_slot, "sending {:?} to client", &message);
    if let Err(err) = transport.send(Some(c_slot), bincode::serialize(&message).unwrap(), false).await {
        tracing::error!(c_slot = c_slot, s_slot = s_slot, "unable to flush rpc callback message");
    } else {
        tracing::trace!(c_slot = c_slot, s_slot = s_slot, "flushed rpc callback message")
    }
}

pub async fn connection_handler(stream: TcpStream, addr: SocketAddr, mut executor: BoxcarExecutor) {
    let transport = Transport::server(stream, addr).await.unwrap();
    let addr = addr.to_string();

    loop {
        let message = transport.recv().await;
        let boxcar_message: BoxcarMessage = bincode::deserialize(&*message.inner).unwrap();
        let response = match boxcar_message {
            BoxcarMessage::RpcReq(req) => {
                match executor.execute_task(req).await {
                    Ok((s_slot, notify)) => {
                        tracing::debug!(address = addr.as_str(), c_slot = message.c_slot, s_slot = s_slot, "successfully scheduled rpc");


                        match executor.get_rpc(s_slot).await {
                            None => panic!("just spawned a task, but executor does not contain the task info"),
                            Some(inner) => {
                                // check the outgoing response. there miiiight be a chance where the RPC could complete
                                // by the time we get here. so, check the result to see if it is BoxcarResult::None
                                match &inner.result {
                                    RpcResult::None => {
                                        tracing::debug!(address = addr.as_str(), c_slot = message.c_slot, s_slot = s_slot, "spawning notify handler for rpc callback");
                                        tokio::task::spawn(rpc_callback(message.c_slot, s_slot, notify, transport.clone(), executor.clone()));
                                    },
                                    _ => {
                                        // yep, the task finished in-between when it was scheduled and we are building the response to the client
                                        // in this case, we don't need to schedule the callback handler
                                        tracing::info!(address = addr.as_str(), c_slot = message.c_slot, s_slot = s_slot, "rpc execution completed already, not scheduling notify handler")
                                    }
                                }
                                BoxcarMessage::RpcRslt((s_slot, inner.result))
                            }
                        }



                    }
                    Err(err) => {
                        tracing::warn!("unable to execute rpc. {:?}", &err);
                        BoxcarMessage::ServerError(format!("unable to execute rpc. {:?}", err))
                    }
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
    inbox: Arc<Queue<BoxcarMessage>>,
    notify: Arc<Notify>
}
impl Session {
    pub async fn call(&self, req: RpcRequest) -> anyhow::Result<()> {
        let message = BoxcarMessage::RpcReq(req);
        tracing::debug!(c_slot = self.c_slot, "{:?}", &message);
        let _q = self.transport.send(Some(self.c_slot), bincode::serialize(&message)?, false).await;

        Ok(())
    }

    /// Deliver a message
    pub async fn deliver(&mut self, message: BoxcarMessage) {
        self.inbox.push(message);
        self.notify.notify_one();
        tracing::trace!(c_slot = self.c_slot, "oh look, i got a message!")
    }

    /// Receive a message from the inbox, waiting for a message to arrive it is empty
    pub async fn recv(&mut self) -> BoxcarMessage {
        self.inbox.pop().await
    }

    /// Try and Receive a message, returning none if the inbox is empty
    pub async fn try_recv(&mut self) -> Option<BoxcarMessage> {
        self.inbox.try_pop()
    }

    /// Probe the status of the RPC
    pub async fn probe(&self) -> anyhow::Result<()> {

        todo!()
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
            loop {
                let message = trx.recv().await;
                let mut session_handle = sesh.write().await;
                let c_slot = message.c_slot;

                // locate the proper session to route the message to
                let handle = session_handle.iter_mut().find(|v| v.c_slot == c_slot);
                if handle.is_none() {
                    tracing::warn!(c_slot = c_slot, "unknown c_slot or no such session exists");
                    continue;
                }

                let decoded = bincode::deserialize(&message.inner);
                let inner = handle.unwrap();
                let msg = decoded.unwrap();
                tracing::trace!(c_slot = c_slot, "delivering message to session inbox. {:?}", &msg);
                inner.deliver(msg).await;
            }
        });

        Self {
            sessions,
            transport
        }
    }

    pub async fn session(&self) -> Session {
        let session = Session {
            c_slot: self.transport.allocate_slot().await,
            transport: self.transport.clone(),
            inbox: Arc::new(Queue::new(32)),
            notify: Arc::new(Default::default())
        };
        self.sessions.write().await.push(session.clone());
        session
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

        fn contains(&self, method: &str) -> bool {
            true
        }

        fn package(&self) -> &str {
            todo!()
        }
    }

    #[tokio::test]
    async fn test_executor() {
        let test_handler = TestHandler {};

        let mut executor = BoxcarExecutor {
            slots: Arc::new(Default::default()),
            subscriber_map: Default::default(),
            handlers: Default::default(),
            tasks: Default::default()
        };
        executor.add_handler(Box::new(test_handler)).await;

        assert_eq!(1, executor.num_handlers().await)

    }

    #[tokio::test]
    async fn test_task_execution() {
        let test_handler = TestHandler {};

        let mut executor = BoxcarExecutor {
            slots: Arc::new(Default::default()),
            subscriber_map: Default::default(),
            handlers: Default::default(),
            tasks: Default::default()
        };
        executor.add_handler(Box::new(test_handler)).await;

        let task = RpcRequest {
            method: "foo".to_string(),
            body: vec![],
            delay: false
        };
        let req = executor.execute_task(task).await;
        assert_eq!(req.is_ok(), true);

        sleep(Duration::from_secs(2)).await;

        tracing::info!("tasks: {:?}", executor.tasks);

        let result_handle = executor.tasks.read().await;
        let result = result_handle.get(&req.unwrap().0).unwrap().read().await;
        match &result.result {
            RpcResult::None | RpcResult::Err(_) => assert_eq!(false, true),
            RpcResult::Ok(inner) => {
                let v: Vec<u8> = vec![];
                assert_eq!(*inner, v);
            },
        }
    }

    #[tokio::test]
    async fn test_initial_packet() {
        tracing_subscriber::fmt::init();
        //
        // create a server
        let test_handler = TestHandler {};
        let mut executor = BoxcarExecutor::new();
        executor.add_handler(Box::new(test_handler)).await;

        let server = Server {
            bind_addr: "127.0.0.1:9002".to_string(),
            executor
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

        sleep(Duration::from_secs(1)).await;

        let mut session = client.session().await;
        let _ = session.call(RpcRequest {
            method: "foo".to_string(),
            body: vec![],
            delay: false
        }).await;

        sleep(Duration::from_micros(120)).await;
        let result = session.try_recv().await;
        assert_eq!(result.is_none(), true);

        sleep(Duration::from_secs(2)).await;
        let result = session.try_recv().await;
        println!("{:?}", &result);



    }

}