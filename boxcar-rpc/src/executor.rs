use crate::rcm::Claim;
use crate::{Handler, RPCTask, RpcRequest, RpcResult};
use anyhow::bail;
use rand::Rng;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, trace};

#[derive(Debug)]
pub struct BusWrapper {
    pub bus: broadcast::Sender<(u16, Option<RpcResult>)>,
    pub s_slot: u16,
    pub resource_claims: Vec<Claim>,
}
impl BusWrapper {
    #[instrument]
    pub fn send(
        &self,
        message: RpcResult,
    ) -> Result<usize, broadcast::error::SendError<(u16, Option<RpcResult>)>> {
        self.bus.send((self.s_slot, Some(message)))
    }
}

type BoxcarBus = (u16, Option<RpcResult>);

#[derive(Clone)]
pub struct BoxcarExecutor {
    /// List of all allocated slots
    pub(crate) slots: Arc<RwLock<Vec<u16>>>,
    /// Map that maps a Transport to all slots it is listening to
    pub(crate) handlers: Arc<RwLock<Vec<Arc<Handler>>>>,
    pub(crate) tasks: Arc<RwLock<BTreeMap<u16, Arc<RwLock<RPCTask>>>>>,
    /// Task Status Updates
    pub(crate) bus: broadcast::Sender<BoxcarBus>,
}
impl BoxcarExecutor {
    #[instrument]
    pub fn new() -> Self {
        let bus: (broadcast::Sender<BoxcarBus>, broadcast::Receiver<BoxcarBus>) =
            broadcast::channel(32);
        let selff = BoxcarExecutor {
            slots: Default::default(),
            handlers: Default::default(),
            tasks: Default::default(),
            bus: bus.0,
        };

        // Record the last message and save it to the Task's struct
        let r_self = selff.clone();
        tokio::task::spawn(async move {
            trace!("starting executor bus reader");
            let mut recv = bus.1;
            loop {
                // loop over every message that comes our way
                while let Ok(message) = recv.recv().await {
                    let reader = r_self.tasks.read().await;
                    // if the s_slot exists (which it should), grab a write handle and set the message
                    if let Some(task_ref) = reader.get(&message.0) {
                        trace!(
                            s_slot = message.0,
                            "received message. writing to task's state"
                        );
                        task_ref.write().await.result = message.1;
                    } else {
                        error!(
                            s_slot = message.0,
                            "received message on internal bus that is for an unknown s_slot"
                        );
                    }
                    drop(reader);
                }
            }
        });

        selff
    }

    /// Add a handler to the executor
    pub async fn add_handler(&mut self, handle: Handler) {
        self.handlers.write().await.push(Arc::new(handle))
    }

    /// Return the number of registered handlers
    #[instrument]
    pub async fn num_handlers(&self) -> usize {
        self.handlers.read().await.len()
    }

    // pub async fn get_rpc(&self, slot: u16) -> Option<RPCTask> {
    //     match self.tasks.read().await.get(&slot) {
    //         None => None,
    //         Some(task) => Some(task.read().await.clone()),
    //     }
    // }

    #[instrument]
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

    /// Return a listener on the bus
    #[instrument]
    pub fn get_listener(&self) -> broadcast::Receiver<(u16, Option<RpcResult>)> {
        self.bus.subscribe()
    }

    /// Run an RpcRequest on the executor
    #[instrument]
    pub async fn execute_task(
        &mut self,
        request: RpcRequest,
        claims: Vec<Claim>,
    ) -> anyhow::Result<u16> {
        let s_slot = self.assign_slot().await;
        let task = RPCTask {
            request,
            result: None,
        };
        debug!(s_slot = s_slot, "executing RPCTask {:?}", &task);
        // let delay = task.request.subscribe;

        // search through registered handles to find the one that contains the requested method
        let handle = self.handlers.read().await;

        trace!(
            s_slot = s_slot,
            "number of registered handlers: {}",
            handle.len()
        );
        let handle = handle
            .iter()
            .find(|v| v.contains(task.request.method.as_str()));

        if handle.is_none() {
            error!(
                s_slot = s_slot,
                method = task.request.method.as_str(),
                "requested handler does not exist"
            );
            bail!("no such handler");
        }

        // build task arc, and insert it into our map
        let task_ref = Arc::new(RwLock::new(task));
        self.tasks.write().await.insert(s_slot, task_ref.clone());

        // handler that will be moved into the closure
        let handler = handle.unwrap().clone();
        // notifier that will be moved into the closure
        let closure_bus = self.bus.clone();
        let closure_slot = s_slot;
        let closure_task_ref = task_ref.clone();
        let bus = BusWrapper {
            bus: self.bus.clone(),
            s_slot,
            resource_claims: claims,
        };
        let closure = async move {
            let task = closure_task_ref.read().await;
            let s_slot = closure_slot;

            // run the handler
            debug!(s_slot = s_slot, "---- entering task handler ----");
            let result = handler
                .call(task.request.method.as_str(), task.request.body.clone(), bus)
                .await;
            debug!(s_slot = s_slot, "---- leaving  task handler ----");
            info!(s_slot = s_slot, "rpc returned {:?}", &result);

            // why is there a drop here? I don't know, but if you remove it- you don't save `result`
            drop(task);

            // take the result of the handler and write it into the RPCTask
            closure_bus.send((s_slot, Some(result)))
            // TODO Release the s_slot from self.slots
        };

        // TODO figure out why spawn_blocking isn't working... or if it's needed
        // if delay {
        //     tracing::trace!(
        //         s_slot = s_slot,
        //         "spawning task on a blocking thread because subscribe is false"
        //     );
        //     tokio::task::spawn_blocking(move || closure);
        // } else {
        //     tracing::trace!(s_slot = s_slot, "spawning task in a non-blocking fashion");
        tokio::task::spawn(closure);
        // }

        Ok(s_slot)
    }
}
impl Default for BoxcarExecutor {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Debug for BoxcarExecutor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "todo",)
    }
}
