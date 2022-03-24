#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use boxcar_rpc::{
        BoxcarExecutor, BusWrapper, HandlerTrait, RpcRequest, RpcResult, WireMessage,
    };
    use boxcar_rpc::{Client, Server};
    use std::time::Duration;
    use tokio::net::TcpListener;
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
    async fn test_basic_rpc() {
        tracing_subscriber::fmt::init();

        let test_handler = TestHandler {};

        let mut executor = BoxcarExecutor::new();
        executor.add_handler(Box::new(test_handler)).await;

        let server = Server::new(TcpListener::bind("127.0.0.1:9933").await.unwrap(), executor);

        tokio::task::spawn(async move { server.serve().await });

        // give the server a minute to start up
        sleep(Duration::from_secs(1)).await;

        let client = Client::new("ws://127.0.0.1:9933").await.unwrap();

        let command = RpcRequest {
            method: "foo".to_string(),
            body: vec![],
            subscribe: true,
        };
        let r = client.call(command).await;
        assert_eq!(r.is_ok(), true);

        let result = r.unwrap();

        sleep(Duration::from_secs(5)).await;
        println!("{:?}", result.try_recv().await)
    }
}
