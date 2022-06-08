use anyhow::bail;
use etcd_client::{Client, GetOptions, PutOptions, SortOrder, SortTarget};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::RwLock;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, instrument, trace, warn, Instrument};

// TODO make this configurable when a service is registered
const ETCD_LEASE_TTL_SEC: u64 = 10;

pub struct ServiceManagerConfig {
    /// Keyspace Base Path
    keyspace: String,
}

impl ServiceManagerConfig {
    pub fn new(keyspace: impl Into<String>) -> Self {
        ServiceManagerConfig {
            keyspace: keyspace.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstanceRegistration {
    /// Service ID
    /// TODO Make this generated
    pub id: String,
    /// Service Address. Can be a IP address or Hostname
    pub addr: IpAddr,
    /// Service Port
    pub port: u16,
    /// Protocol
    pub proto: String,
    /// Meta fields
    pub meta: HashMap<String, String>,

    pub resources: HashMap<String, usize>,
}
impl InstanceRegistration {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            addr: IpAddr::from_str("127.0.0.1").unwrap(),
            port: 0,
            proto: "tcp".to_string(),
            meta: Default::default(),
            resources: Default::default(),
        }
    }
    pub fn id(self, id: String) -> Self {
        Self { id, ..self }
    }
    pub fn addr(self, addr: IpAddr) -> Self {
        Self { addr, ..self }
    }
    pub fn port(self, port: u16) -> Self {
        Self { port, ..self }
    }
    pub fn proto(self, proto: String) -> Self {
        Self { proto, ..self }
    }
    pub fn meta(self, meta: HashMap<String, String>) -> Self {
        Self { meta, ..self }
    }
    pub fn resources(self, resources: HashMap<String, usize>) -> Self {
        Self { resources, ..self }
    }
}
impl Default for InstanceRegistration {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents a registered instance.
/// On drop, the instance is unregistered.
/// The .update method can be used to update the meta key/value store inside the service registration
pub struct RegistrationHandle {
    client: Client,
    path: String,
    lease_id: i64,
    instance: Arc<Mutex<InstanceRegistration>>,
    lease_enabled: Arc<RwLock<bool>>,
}
impl RegistrationHandle {
    async fn new(
        client: Client,
        lease_id: i64,
        path: String,
        instance: InstanceRegistration,
    ) -> anyhow::Result<RegistrationHandle> {
        let lease_enabled = Arc::new(RwLock::new(true));

        let lease_control = lease_enabled.clone();
        let mut client_control = client.clone();
        tokio::task::spawn(async move {
            let (mut keeper, mut stream) = client_control.lease_keep_alive(lease_id).await.unwrap();
            debug!(lease = lease_id, "lease keepalive start");

            let keepalive_time = ETCD_LEASE_TTL_SEC / 2;

            loop {
                if !*lease_control.read().unwrap() {
                    debug!(
                        lease = lease_id,
                        "lease_control set to false, exiting keep alive loop"
                    );
                    break;
                }
                // send a keep alive request
                if let Err(e) = keeper.keep_alive().await {
                    warn!(
                        lease = lease_id,
                        "unable to send keep alive message. {:?}", e
                    )
                    // TODO after too many consecutive failures, kill the server.
                } else {
                    // stream the response
                    if let Some(resp) = stream.message().await.unwrap() {
                        trace!(
                            lease = lease_id,
                            ttl = resp.ttl(),
                            "received keepalive response"
                        );
                    } else {
                        warn!(lease = lease_id, "unable to read response from the stream");
                    }
                }
                // sleep before sending the next ping
                sleep(Duration::from_secs(keepalive_time)).await;
            }
        });

        Ok(RegistrationHandle {
            client,
            path,
            lease_id,
            lease_enabled,
            instance: Arc::new(Mutex::new(instance)),
        })
    }
    /// Update the Service InstanceRegistration
    pub async fn update(&mut self, instance: InstanceRegistration) -> anyhow::Result<()> {
        let val = {
            let mut handle = self.instance.lock().unwrap();
            if *handle == instance {
                debug!(
                    key = self.path.as_str(),
                    lease = self.lease_id,
                    "InstanceRegistration identical. No changes to commit to etcd"
                );
                return Ok(());
            } else {
                *handle = instance;
            }

            serde_json::to_string(&*handle)?
        };

        // TODO Do this in a transaction and track the key version to make sure it doesn't change
        //      underneath us
        let opts = PutOptions::new().with_lease(self.lease_id);
        let _ = self.client.put(self.path.clone(), val, Some(opts)).await?;

        Ok(())
    }

    pub fn peak(&self) -> InstanceRegistration {
        self.instance.lock().unwrap().clone()
    }

    /// Deregister the service. Same as dropping the handle
    pub fn deregister(&self) {
        *self.lease_enabled.write().unwrap() = false;
    }

    /// Return if the lease is active or not
    pub fn active(&self) -> bool {
        *self.lease_enabled.read().unwrap()
    }
}
impl Drop for RegistrationHandle {
    fn drop(&mut self) {
        debug!(
            key = self.path.as_str(),
            lease = self.lease_id,
            "RegistrationHandle dropped, signaling for the lease task to stop"
        );
        *self.lease_enabled.write().unwrap() = false;
    }
}

#[instrument(skip(client, path))]
async fn get_keys(
    mut client: Client,
    path: impl Into<Vec<u8>>,
) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    Ok(client
        .get(
            path,
            Some(
                GetOptions::new()
                    .with_prefix()
                    .with_sort(SortTarget::Create, SortOrder::Ascend),
            ),
        )
        .instrument(tracing::trace_span!("read_keys"))
        .await?
        .kvs()
        .iter()
        .map(|kv| (kv.key().to_vec(), kv.value().to_vec()))
        .collect())
}

pub struct ServiceManager {
    client: Client,
    config: ServiceManagerConfig,
}
impl ServiceManager {
    pub async fn new(
        client: Client,
        config: ServiceManagerConfig,
    ) -> anyhow::Result<ServiceManager> {
        Ok(ServiceManager { client, config })
    }

    /// Registers the given instance into etcd, returning a RegistrationHandle.
    /// TODO Support another type of instance- one that has no ser  ver IP/Port, but has an
    ///      etcdmq instance for tx/rx
    // #[instrument(skip(self))]
    pub async fn register(
        &mut self,
        service: impl Into<String>,
        instance: InstanceRegistration,
    ) -> anyhow::Result<RegistrationHandle> {
        // {keyspace}/{service}/{instance id}
        let key = format!(
            "{}/{}/{}",
            self.config.keyspace,
            &service.into(),
            &instance.id
        );

        let lease = self
            .client
            .lease_grant(ETCD_LEASE_TTL_SEC as i64, None)
            .await?;
        debug!(
            key = key.as_str(),
            lease = lease.id(),
            ttl = lease.ttl(),
            "lease generated"
        );

        let val = serde_json::to_string(&instance)?;
        trace!(
            lease = lease.id(),
            key = key.as_str(),
            "key contents: {}",
            &val
        );

        // create a put option object with our lease ID to associate the two
        // the key is present only when the lease is active- so no lease, no key.
        let opts = PutOptions::new().with_lease(lease.id());
        let _ = self.client.put(key.clone(), val, Some(opts)).await?;

        let lease_id = lease.id();
        RegistrationHandle::new(self.client.clone(), lease_id, key, instance).await
    }

    /// Lookup Instances
    pub async fn lookup(
        &mut self,
        service: impl Into<String>,
    ) -> anyhow::Result<Vec<InstanceRegistration>> {
        // TODO this would be a good case of parallel iter
        Ok(get_keys(
            self.client.clone(),
            format!("{}/{}", self.config.keyspace, &service.into()),
        )
        .await?
        .iter()
        // TODO handle the deserialization errors
        .map(|val| serde_json::from_slice(&*val.1).unwrap())
        .collect())
    }

    /// Return information on a specific instance of a service
    pub async fn query(
        &mut self,
        service: impl Into<String>,
        instance_id: impl Into<String>,
    ) -> anyhow::Result<InstanceRegistration> {
        let r = self
            .client
            .get(
                format!(
                    "{}/{}/{}",
                    self.config.keyspace,
                    &service.into(),
                    &instance_id.into()
                ),
                None,
            )
            .await?;

        if let Some(key) = r.kvs().first() {
            Ok(serde_json::from_slice(key.value())?)
        } else {
            bail!("instance not found")
        }
    }

    /// Starts a watcher on the given service name, causing the results for `ServiceManager.lookup`
    /// to be cached- avoiding the call to etcd to get the services
    pub async fn watch(&mut self, _service: impl Into<String>) -> anyhow::Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use crate::{InstanceRegistration, ServiceManager, ServiceManagerConfig, ETCD_LEASE_TTL_SEC};
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_svc_query() {
        let client = etcd_client::Client::connect(["localhost:2379"], None)
            .await
            .unwrap();

        #[warn(unused_mut)]
        let mut svc = ServiceManager::new(
            client,
            ServiceManagerConfig {
                keyspace: "test".to_string(),
            },
        )
        .await
        .unwrap();

        let hsvc = svc
            .register(
                "hello_world2",
                InstanceRegistration {
                    id: "id".to_string(),
                    addr: "127.0.0.2".to_string().parse().unwrap(),
                    port: 0,
                    proto: "tcp".to_string(),
                    meta: Default::default(),
                    resources: Default::default(),
                },
            )
            .await;

        assert_eq!(hsvc.is_ok(), true);

        let handle = hsvc.unwrap();

        svc.query("hello_world2", "id").await;
    }

    #[tokio::test]
    async fn test_reg() {
        let client = etcd_client::Client::connect(["localhost:2379"], None)
            .await
            .unwrap();

        #[warn(unused_mut)]
        let mut svc = ServiceManager::new(
            client,
            ServiceManagerConfig {
                keyspace: "test".to_string(),
            },
        )
        .await
        .unwrap();

        let hsvc = svc
            .register(
                "hello_world",
                InstanceRegistration {
                    id: "id".to_string(),
                    addr: "127.0.0.1".to_string().parse().unwrap(),
                    port: 0,
                    proto: "tcp".to_string(),
                    meta: Default::default(),
                    resources: Default::default(),
                },
            )
            .await;

        assert_eq!(hsvc.is_ok(), true);

        let handle = hsvc.unwrap();

        sleep(Duration::from_secs(2)).await;

        let lookup = svc.lookup("hello_world").await;
        assert_eq!(lookup.is_ok(), true);
        let services = lookup.unwrap();
        assert_eq!(services.len(), 1);

        // drop the handle, releasing it
        drop(handle);

        // let the lease expire
        sleep(Duration::from_secs(ETCD_LEASE_TTL_SEC + 1)).await;

        // now, re-check to see if it's gone
        let lookup = svc.lookup("hello_world").await;
        assert_eq!(lookup.is_ok(), true);
        let services = lookup.unwrap();
        assert_eq!(services.len(), 0);
    }
}
