#[cfg(test)]
mod tests {
    use crate::{InstanceRegistration, RegistrationHandle, ServiceManager, ServiceManagerConfig};
    use std::time::Duration;
    use tokio::time::sleep;
    use tracing::info;

    #[tokio::test]
    async fn test_eg() {}
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
//     mut client: Client,
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
