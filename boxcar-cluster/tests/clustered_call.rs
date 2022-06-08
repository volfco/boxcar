#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use boxcar_cluster::{Client, ClusteredCallConfig, Service};
    use boxcar_rpc::{BoxcarExecutor, BusWrapper, HandlerTrait, RpcResult};
    use boxcar_rpc::{RpcRequest, Server};
    use etcdsvc::ServiceManagerConfig;
    use std::collections::HashMap;
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
    async fn test_two_node_query() {
        tracing_subscriber::fmt::init();

        let etcd_client = etcd_client::Client::connect(["localhost:2379"], None)
            .await
            .unwrap();

        let test_handler = TestHandler {};

        let mut executor = BoxcarExecutor::new();
        executor.add_handler(Box::new(test_handler)).await;

        let server_a = Server::new()
            .bind("127.0.0.1:9803".parse().unwrap())
            .executor(executor.clone());
        let mut service_a = Service::new("boxcar-testing", server_a, etcd_client.clone())
            .await
            .unwrap();
        tokio::task::spawn(async move { service_a.serve().await });

        let server_b = Server::new()
            .bind("127.0.0.1:9802".parse().unwrap())
            .executor(executor.clone());
        let mut service_b = Service::new("boxcar-testing", server_b, etcd_client.clone())
            .await
            .unwrap();
        tokio::task::spawn(async move { service_b.serve().await });

        // sleep for a second to make sure everything registers and comes up
        sleep(Duration::from_secs(1)).await;

        let cluster_client = Client::new(etcd_client, "boxcar-testing").await;
        assert_eq!(cluster_client.is_ok(), true);

        let command = RpcRequest {
            method: "foo".to_string(),
            body: vec![],
            subscribe: true,
            resources: HashMap::new(),
        };

        let mut cc = cluster_client.unwrap();
        let req = cc.call(command, ClusteredCallConfig::new()).await;
    }
}
