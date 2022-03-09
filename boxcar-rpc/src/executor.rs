use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use rand::Rng;
use tokio::sync::{Notify, RwLock};

use anyhow::bail;

use crate::{Handler, RpcRequest, Transport, RpcResult, RPCTask};

#[derive(Clone)]
pub struct BoxcarExecutor {
    /// List of all allocated slots
    pub(crate) slots: Arc<RwLock<Vec<u16>>>,
    /// Map that maps a Transport to all slots it is listening to
    pub(crate) subscriber_map: HashMap<Transport, Vec<u16>>,
    pub(crate) handlers: Arc<RwLock<Vec<Arc<Handler>>>>,
    pub(crate) tasks: Arc<RwLock<BTreeMap<u16, Arc<RwLock<RPCTask>>>>>
}
impl BoxcarExecutor {
    pub fn new() -> Self {
        BoxcarExecutor {
            slots: Arc::new(Default::default()),
            subscriber_map: Default::default(),
            handlers: Default::default(),
            tasks: Default::default()
        }
    }

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

    pub async fn execute_task(&mut self, request: RpcRequest) -> anyhow::Result<(u16, Arc<Notify>)> {
        let s_slot = self.assign_slot().await;
        let task = RPCTask {
            slot: s_slot,
            request,
            result: RpcResult::None
        };
        tracing::debug!(s_slot = s_slot, "executing RPCTask {:?}", &task);
        let delay = task.request.delay;

        // search through registered handles to find the one that contains the requested method
        let handle = self
            .handlers.read().await;

        tracing::trace!(s_slot = s_slot, "number of registered handlers: {}", handle.len());
        let handle = handle
            .iter()
            .find(|v| v.contains(task.request.method.as_str()));

        if handle.is_none() {
            tracing::error!(s_slot = s_slot, method = task.request.method.as_str(), "requested handler does not exist");
            bail!("no such handler");
        }

        let notifier = Arc::new(Notify::new());

        // build task arc, and insert it into our map
        let task_ref = Arc::new(RwLock::new(task));
        self.tasks.write().await.insert(s_slot, task_ref.clone());

        // handler that will be moved into the closure
        let handler = handle.unwrap().clone();
        // notifier that will be moved into the closure
        let closure_notify = notifier.clone();
        let closure = async move {
            let task_ref = task_ref.clone();

            // pull out the operation information, as we don't want to move stuff into our handler
            let task = task_ref.read().await;
            let s_slot = task.slot;

            // // run the handler
            tracing::debug!(s_slot = s_slot, "---- entering task handler ----");
            let result = handler.call(task.request.method.as_str(), task.request.body.clone()).await;
            tracing::debug!(s_slot = s_slot, "---- leaving  task handler ----");
            tracing::info!(s_slot = s_slot, "rpc returned {:?}", &result);

            // why is there a drop here? I don't know, but if you remove it- you don't save `result`
            drop(task);

            // take the result of the handler and write it into the RPCTask
            task_ref.write().await.result = result;
            closure_notify.notify_one();
        };

        if delay {
            tracing::trace!(s_slot = s_slot, "spawning task on a blocking thread");
            tokio::task::spawn_blocking(move || closure);
        } else {
            tracing::trace!(s_slot = s_slot, "spawning task in a non-blocking fashion");
            tokio::task::spawn(closure);
        }

        Ok((s_slot, notifier))
    }

}