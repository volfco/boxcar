#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use boxcar_cluster::Service;
    use boxcar_rpc::Server;
    use boxcar_rpc::{BoxcarExecutor, BusWrapper, HandlerTrait, RpcResult};
    use etcdsvc::ServiceManagerConfig;
    use std::time::Duration;
    use tokio::time::sleep;

    struct TestHandler {}
    #[async_trait]
    impl HandlerTrait for TestHandler {
        async fn call(&self, _method: &str, _arguments: Vec<u8>, _bus: BusWrapper) -> RpcResult {
            tracing::info!("handler has started");
            sleep(Duration::from_secs(1)).await;
            RpcResult::Ok(vec![])
        }

        fn contains(&self, _method: &str) -> bool {
            true
        }

        fn package(&self) -> &str {
            todo!()
        }
    }

    #[tokio::test]
    async fn test_two_node_reg() {
        tracing_subscriber::fmt::init();

        let client = etcd_client::Client::connect(["localhost:2379"], None)
            .await
            .unwrap();

        let test_handler = TestHandler {};

        let mut executor = BoxcarExecutor::new();
        executor.add_handler(Box::new(test_handler)).await;

        let server_a = Server::new()
            .bind("127.0.0.1:9801".parse().unwrap())
            .executor(executor.clone());
        let mut service_a = Service::new("boxcar-testing-two", server_a, client.clone())
            .await
            .unwrap();
        tokio::task::spawn(async move { service_a.serve().await });

        let server_b = Server::new()
            .bind("127.0.0.1:9801".parse().unwrap())
            .executor(executor.clone());
        let mut service_b = Service::new("boxcar-testing-two", server_b, client.clone())
            .await
            .unwrap();
        tokio::task::spawn(async move { service_b.serve().await });

        // sleep for a second to make sure everything registers and comes up
        sleep(Duration::from_secs(1)).await;

        let mut service_manager = etcdsvc::ServiceManager::new(
            client.clone(),
            ServiceManagerConfig::new("boxcar-cluster"),
        )
        .await
        .unwrap();

        let lookup = service_manager.lookup("boxcar-testing-two").await;
        assert_eq!(lookup.is_ok(), true);
        let services = lookup.unwrap();
        assert_eq!(services.len(), 2);
    }
}
