use boxcar_rpc::rcm::ResourceManager;
use etcdsvc_rs::{RegistrationHandle, ServiceManagerConfig};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, trace, warn};

const METADATA_REFRESH_INTERVAL: u64 = 6;

pub struct Service {
    name: String,
    service_manager: etcdsvc_rs::ServiceManager,
    server: boxcar_rpc::Server,
    metadata: HashMap<String, String>,
}
impl Service {
    pub async fn new(
        name: impl Into<String>,
        server: boxcar_rpc::Server,
        etcd_client: etcd_client::Client,
    ) -> anyhow::Result<Service> {
        let service_manager = etcdsvc_rs::ServiceManager::new(
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
        let instance_config = etcdsvc_rs::InstanceRegistration::new()
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

#[cfg(test)]
mod tests {
    use crate::Service;

    #[tokio::test]
    async fn test_foo() {
        // let manager = Service::new().await;
    }
}
