use etcd_client::PutOptions;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, trace, warn};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::time::sleep;

// TODO make this configurable when a service is registered
const ETCD_LEASE_TTL_SEC: u64 = 30;

struct ServiceManagerConfig {
    /// Keyspace Base Path
    keyspace: String,
}
impl ServiceManagerConfig {
    pub fn new(client: etcd_client::Client, service: String) -> Self {
        ServiceManagerConfig {
            keyspace: "".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct InstanceRegistration {
    /// Service ID
    /// TODO Make this generated
    id: String,
    /// Service Address. Can be a IP address or Hostname
    addr: String,
    /// Service Port
    port: u16,
    /// Meta fields
    meta: HashMap<String, String>,
}

/// Represents a registered instance.
/// On drop, the instance is unregistered.
/// The .update method can be used to update the meta key/value store inside the service registration
struct RegistrationHandle {
    lease_enabled: Arc<RwLock<bool>>,
}
impl RegistrationHandle {
    async fn new(
        mut client: etcd_client::Client,
        lease_id: i64,
    ) -> anyhow::Result<RegistrationHandle> {
        let lease_enabled = Arc::new(RwLock::new(true));

        let lease_control = lease_enabled.clone();
        tokio::task::spawn(async move {
            let (mut keeper, mut stream) = client.lease_keep_alive(lease_id).await.unwrap();
            debug!(lease = lease_id, "lease keepalive start");

            let keepalive_time = ETCD_LEASE_TTL_SEC / 2;

            loop {
                if *lease_control.read().await == false {
                    info!("lease_control set to false, exiting keep alive loop");
                    break;
                }
                // send a keep alive request
                if let Err(e) = keeper.keep_alive().await {
                    warn!("unable to send keep alive message")
                    // TODO after too many consecutive failures, kill the server.
                } else {
                    // stream the response
                    if let Some(resp) = stream.message().await.unwrap() {
                        debug!("lease {:?} keep alive, new ttl {:?}", resp.id(), resp.ttl());
                    } else {
                        warn!("unable to read response from the stream");
                    }
                }
                // sleep before sending the next ping
                sleep(Duration::from_secs(keepalive_time)).await;
            }
        });

        Ok(RegistrationHandle { lease_enabled })
    }
    async fn update(&self, key: String, val: String) {}
}
// impl Drop for RegistrationHandle {
//     fn drop(&mut self) {
//         debug!("RegistrationHandle dropped, signaling for the lease task to stop");
//         *self.lease_enabled.blocking_write() = false;
//     }
// }

struct ServiceManager {
    client: etcd_client::Client,
    config: ServiceManagerConfig,
}
impl ServiceManager {
    pub async fn new(
        client: etcd_client::Client,
        config: ServiceManagerConfig,
    ) -> anyhow::Result<ServiceManager> {
        Ok(ServiceManager { client, config })
    }

    /// Registers the given instance into etcd, returning a RegistrationHandle.
    /// TODO Support another type of instance- one that has no server IP/Port, but has an
    ///      etcdmq instance for tx/rx
    // #[instrument(skip(self))]
    pub async fn register(
        &mut self,
        service: impl Into<String>,
        instance: InstanceRegistration,
    ) -> anyhow::Result<RegistrationHandle> {
        let lease = self
            .client
            .lease_grant(ETCD_LEASE_TTL_SEC as i64, None)
            .await?;
        debug!(lease = lease.id(), ttl = lease.ttl(), "lease generated");

        // {keyspace}/{service}/{instance id}
        let key = format!(
            "{}/{}/{}/_meta",
            self.config.keyspace,
            &service.into(),
            &instance.id
        );
        let val = serde_json::to_string(&instance)?;
        trace!(key = key.as_str(), "key contents: {}", &val);

        // create a put option object with our lease ID to associate the two
        // the key is present only when the lease is active- so no lease, no key.
        let opts = PutOptions::new().with_lease(lease.id());
        let put_rq = self.client.put(key, val, Some(opts)).await?;

        info!("{:?}", put_rq);

        let lease_id = lease.id();
        RegistrationHandle::new(self.client.clone(), lease_id).await
    }
}

#[cfg(test)]
mod tests {
    use crate::{InstanceRegistration, RegistrationHandle, ServiceManager, ServiceManagerConfig};
    use etcd_client::Client;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_reg() {
        tracing_subscriber::fmt::init();

        let mut client = Client::connect(["localhost:2379"], None).await.unwrap();

        let mut svc = ServiceManager::new(
            client,
            ServiceManagerConfig {
                keyspace: "test".to_string(),
            },
        )
        .await
        .unwrap();

        svc.register(
            "hello_world",
            InstanceRegistration {
                id: "id".to_string(),
                addr: "".to_string(),
                port: 0,
                meta: Default::default(),
            },
        )
        .await;

        sleep(Duration::from_secs(60)).await;
    }
}

// #[derive(Debug, Clone)]
// pub struct ResourceManager {
//     resources: Arc<RwLock<HashMap<String, serde_json::Value>>>,
// }
// impl ResourceManager {}
//
// pub struct ClusterSerer {
//     server: boxcar_rpc::Server,
//     resources: ResourceManager,
// }
// impl ClusterSerer {
//     pub fn new(listener: TcpListener, executor: BoxcarExecutor) -> anyhow::Result<Self> {
//         let addr = &listener.local_addr()?;
//
//         let s = ClusterSerer {
//             server: boxcar_rpc::Server::new(listener, executor),
//             resources: ResourceManager {},
//         };
//
//         Ok(s)
//     }
// }
//
// /// Keep the key used for discovery present.
// ///
// /// By getting a lease grant and attaching it to a key, we can keep the key alive as long as the client is alive.
// async fn register(
//     mut client: etcd_client::Client,
//     base_key: String,
//     id: String,
//     addr: &SocketAddr,
// ) -> anyhow::Result<()> {
//     // generate a etcd lease
//     let lease = client.lease_grant(ETCD_LEASE_TTL_SEC, None).await?;
//     trace!(lease = lease.id(), ttl = lease.ttl(), "lease generated");
//
//     // create a key in /<base>/<addr>, whee is like /foo/bar/server attached to lease
//     let key = format!("{}/{}", base_key, id);
//     let val = format!("{}:{}", addr.ip(), addr.port());
//     // TODO attach lease to kv
//     let kv = client.put(key, val, None).await?;
//
//     debug!(
//         lease = lease.id(),
//         key = key,
//         value = val,
//         "successfully created registration"
//     );
//
//     // loop (lease time * 0.87) seconds to re-accuire the lease
//     let sleep_duration = ETCD_LEASE_TTL_SEC - 3;
//     tokio::spawn(async move {
//         let (mut keeper, mut stream) = client.lease_keep_alive(lease.id()).await?;
//     });
//
//     Ok(())
// }
//
// struct ClusteredServerTarget {
//     id: String,
//     address: (IpAddr, u16),
//     client: Option<boxcar_rpc::Client>,
// }
// impl ClusteredServerTarget {
//     async fn is_open() {}
//     async fn open() {}
//     async fn close() {}
// }
//
// struct ClusteredClient {
//     /// Vec of ClusterServer targets read from etcd
//     known_servers: Arc<RwLock<Vec<ClusteredServerTarget>>>,
// }

// sabigenkicallieruby