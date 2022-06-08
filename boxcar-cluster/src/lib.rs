use boxcar_rpc::rcm::ResourceManager;
use boxcar_rpc::{BoxcarMessage, RpcRequest};
use etcdsvc::{InstanceRegistration, RegistrationHandle, ServiceManager, ServiceManagerConfig};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, trace, warn};

const METADATA_REFRESH_INTERVAL: u64 = 6; // seconds

pub struct Service {
    name: String,
    service_manager: ServiceManager,
    server: boxcar_rpc::Server,
    metadata: HashMap<String, String>,
}
impl Service {
    pub async fn new(
        name: impl Into<String>,
        server: boxcar_rpc::Server,
        etcd_client: etcd_client::Client,
    ) -> anyhow::Result<Service> {
        let service_manager = etcdsvc::ServiceManager::new(
            etcd_client.clone(),
            ServiceManagerConfig::new("boxcar-cluster"),
        )
        .await?;

        Ok(Service {
            name: name.into(),
            service_manager,
            server,
            metadata: HashMap::new(),
        })
    }

    pub fn set_metadata(self, metadata: HashMap<String, String>) -> Self {
        Self { metadata, ..self }
    }

    pub async fn serve(&mut self) -> anyhow::Result<()> {
        let instance_config = etcdsvc::InstanceRegistration::new()
            .addr(self.server.bind.ip())
            .port(self.server.bind.port())
            .meta(self.metadata.clone())
            .resources(self.server.resource_manager.peak());

        let service = self
            .service_manager
            .register(self.name.clone(), instance_config)
            .await?;

        tokio::task::spawn(lease_metadata_updater(
            self.server.resource_manager.clone(),
            service,
        ))
        .await?;

        self.server.serve().await
    }
}

// periodically try and update the resource information in the service metadata
async fn lease_metadata_updater(
    resource_manager: ResourceManager,
    mut service_handle: RegistrationHandle,
) {
    loop {
        sleep(Duration::from_secs(METADATA_REFRESH_INTERVAL)).await;

        if !service_handle.active() {
            debug!(
                "service registration handle is no longer active. shutting down metadata_updater"
            );
            break;
        }

        if let Err(err) = service_handle
            .update(service_handle.peak().resources(resource_manager.peak()))
            .await
        {
            warn!(
                "unable to update service instance registration information. {:?}",
                err
            );
        } else {
            trace!("successfully updated instance registration information");
        }
    }
}

pub struct Client {
    etcd: etcd_client::Client,
    service: String,
    service_manager: ServiceManager,
    clients: HashMap<String, boxcar_rpc::Client>,
}
impl Client {
    pub async fn new(
        etcd: etcd_client::Client,
        service: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let service_manager =
            ServiceManager::new(etcd.clone(), ServiceManagerConfig::new("testing")).await?;

        Ok(Client {
            etcd,
            service: service.into(),
            service_manager,
            clients: Default::default(),
        })
    }
    pub async fn call(
        &mut self,
        inner: RpcRequest,
        config: ClusteredCallConfig,
    ) -> anyhow::Result<(String, u16)> {
        let mut targets = self
            .service_manager
            .lookup(&self.service)
            .await?
            .into_iter()
            // TODO Can we do both filter & sort in one?
            .filter(|v| filter_targets(v, &inner))
            .collect::<Vec<InstanceRegistration>>();

        targets.sort_by(|a, b| sort_targets(&inner, a, b));

        trace!("sorted targets: {:?}", &targets);

        let target = if config.dense_pack {
            &targets[0]
        } else {
            &targets[targets.len() - 1]
        };

        // get the client, if one exists
        let target_client = if let Some(client) = self.clients.get(&target.id) {
            trace!(target = &target.id.as_str(), "connection cached");
            client.clone()
        } else {
            trace!(
                target = &target.id.as_str(),
                "connection not cached. attempting to establish one"
            );

            // TODO move onto the next target if this fails
            let new_client =
                boxcar_rpc::Client::new(format!("{}:{}", &target.addr, &target.port).as_str())
                    .await?;
            self.clients.insert(target.id.clone(), new_client.clone());
            new_client
        };

        Ok((target.id.clone(), target_client.call(inner).await?))
    }

    pub async fn recv(&mut self, slot: (String, u16)) -> anyhow::Result<BoxcarMessage> {
        // figure out which client to pass the s_slot to
        let client = if let Some(client) = self.clients.get(&slot.0) {
            client.clone()
        } else {
            let target = self.service_manager.query(&self.service, slot.0).await?;
            let new_client =
                boxcar_rpc::Client::new(format!("{}:{}", &target.addr, &target.port).as_str())
                    .await?;
            self.clients.insert(target.id.clone(), new_client.clone());
            new_client
        };

        client.recv(slot.1).await
    }
}

fn filter_targets(v: &InstanceRegistration, inner: &RpcRequest) -> bool {
    let mut matched = true;

    // match every resource in the request to the target node
    for resource in &inner.resources {
        matched = matched
            && match v.resources.get(resource.0.as_str()) {
                None => false,
                Some(amnt) => amnt >= resource.1,
            }
    }
    matched
}

/// Compare target instances. Forward order is sparsely packed instances. Reverse order is Densely Packed
fn sort_targets(
    inner: &RpcRequest,
    a: &InstanceRegistration,
    b: &InstanceRegistration,
) -> Ordering {
    fn get_resource_pct(req: &RpcRequest, v: &InstanceRegistration) -> u8 {
        let mut pct = 1.0;
        for resource in &req.resources {
            pct *= *resource.1 as f64 / *v.resources.get(resource.0).unwrap() as f64;
        }
        (pct * 100.0) as u8
    }
    Ord::cmp(&get_resource_pct(inner, a), &get_resource_pct(inner, b))
}

pub struct ClusteredCallConfig {
    dense_pack: bool,
}
impl ClusteredCallConfig {
    pub fn new() -> Self {
        Self { dense_pack: true }
    }
}

#[cfg(test)]
mod tests {
    use crate::{filter_targets, sort_targets};
    use boxcar_rpc::RpcRequest;
    use etcdsvc::InstanceRegistration;
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::str::FromStr;

    fn get_instances() -> Vec<InstanceRegistration> {
        let mut a_resources = HashMap::new();
        a_resources.insert("foo".to_string(), 1024 as usize);
        a_resources.insert("bar".to_string(), 2048 as usize);

        let mut b_resources = HashMap::new();
        b_resources.insert("foo".to_string(), 2048 as usize);
        b_resources.insert("bar".to_string(), 4096 as usize);

        let mut c_resources = HashMap::new();
        c_resources.insert("foo".to_string(), 512 as usize);
        c_resources.insert("bar".to_string(), 1024 as usize);

        vec![
            InstanceRegistration {
                id: "a".to_string(),
                addr: IpAddr::from_str("127.0.0.1").unwrap(),
                port: 1,
                proto: "".to_string(),
                meta: Default::default(),
                resources: a_resources,
            },
            InstanceRegistration {
                id: "b".to_string(),
                addr: IpAddr::from_str("127.0.0.1").unwrap(),
                port: 1,
                proto: "".to_string(),
                meta: Default::default(),
                resources: b_resources,
            },
            InstanceRegistration {
                id: "c".to_string(),
                addr: IpAddr::from_str("127.0.0.1").unwrap(),
                port: 1,
                proto: "".to_string(),
                meta: Default::default(),
                resources: c_resources,
            },
        ]
    }

    #[test]
    fn test_sparse_packed_instances() {
        let mut resources = HashMap::new();
        resources.insert("foo".to_string(), 512 as usize);
        resources.insert("bar".to_string(), 512 as usize);

        let req = RpcRequest {
            method: "".to_string(),
            body: vec![],
            subscribe: false,
            resources,
        };

        let mut targets = get_instances();
        targets.sort_by(|a, b| sort_targets(&req, a, b));

        assert_eq!(targets[0].id, "b".to_string());
    }

    #[test]
    fn test_filter_instances() {
        let mut resources = HashMap::new();
        resources.insert("foo".to_string(), 1024 as usize);
        resources.insert("bar".to_string(), 1024 as usize);

        let req = RpcRequest {
            method: "".to_string(),
            body: vec![],
            subscribe: false,
            resources,
        };

        let mut targets = get_instances()
            .into_iter()
            .filter(|v| filter_targets(v, &req))
            .collect::<Vec<InstanceRegistration>>();

        // this should filter out one instance
        assert_eq!(targets.len(), 2);
    }

    #[tokio::test]
    async fn test_foo() {
        // let manager = Service::new().await;
    }
}
