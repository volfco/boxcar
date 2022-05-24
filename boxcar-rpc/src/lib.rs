extern crate core;

pub mod client;
///
///
///
///
/// ## Terms
/// `c_slot` Client Communication Slot.
/// `s_slot` Server Slot
pub mod executor;
pub mod rcm;
pub mod server;
pub mod utils;

pub use crate::client::Client;
pub use crate::executor::{BoxcarExecutor, BusWrapper};
pub use crate::server::Server;
use std::collections::HashMap;

// TODO Implement RPC Result expiration on number of rpcs completed. By default purge after (2*max concurrent tasks) stored RPCResults

use crate::rcm::ResourceError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::error::Error;

pub trait BoxcarMessageTrait {
    fn encode(&self, buf: &mut Vec<u8>) -> Result<(), Box<dyn Error>>;
    fn decode(&self, buf: Vec<u8>) -> Result<(), Box<dyn Error>>;
}

#[async_trait]
pub trait HandlerTrait {
    async fn call(&self, method: &str, arguments: Vec<u8>, bus: BusWrapper) -> RpcResult;
    fn contains(&self, _: &str) -> bool;
    fn package(&self) -> &str;
}

pub type Handler = Box<dyn HandlerTrait + Sync + Send + 'static>;
pub type Callback = Box<dyn Fn(&str) -> bool + Sync + Send>;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
/// TODO Make this into something better
pub struct WireMessage {
    pub c_slot: u16,
    pub inner: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BoxcarMessage {
    /// Initiate an RPC
    RpcReq(RpcRequest),

    /// RPC Request Result
    /// When sent from the server -> client, signifies that the RpcReq was accepted and this is the
    /// associated s_slot.
    /// When sent from the Client to the Server, signifies that the client is requesting the last
    /// Rpc Result
    RpcReqRslt(u16),

    /// RPC Status
    RpcRslt((u16, Option<RpcResult>)),

    /// Subscribe to the specified s_slot
    Sub(Vec<u16>),
    /// Unsubscribe from the specified s_slot
    UnSub(Vec<u16>),
    /// (un)subscribe operation success. bool indicates if the operation resulted in any changes
    SubOpFin(bool),

    ResourceError(ResourceError),

    ServerError(String),
    Hangup,
    Ping(u8),
    Pong(u8),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RpcRequest {
    /// Remote method to invoke
    pub method: String,
    /// Raw body that is passed to the method handler. This can be any arbitrary data that would be
    /// decoded on the server's side
    pub body: Vec<u8>,
    /// Ask the server to automatically send a response to the client when it happens. Reduces
    /// latency and overhead.
    pub subscribe: bool,
    /// An optional map of resources that are required to execute this RPC call.
    pub resources: HashMap<String, usize>,
}

/// enum to represent the state of an RPC
#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub enum RpcResult {
    None,
    Ok(Vec<u8>),
    Err(Vec<u8>),
}

/// Represents an executing RPC Job
#[derive(Clone, Debug)]
pub struct RPCTask {
    request: RpcRequest,
    result: Option<RpcResult>,
}
