extern crate core;

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use boxcar_rpc::{
        BoxcarExecutor, BoxcarMessage, BusWrapper, HandlerTrait, RpcRequest, RpcResult,
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

        sleep(Duration::from_secs(1)).await;

        let resp = client.recv(result).await;
        if let Ok(inner) = resp {
            if let BoxcarMessage::RpcRslt(inner) = inner {
                if let RpcResult::Ok(buf) = inner.1 {
                    assert_eq!(buf.len(), 0);
                } else {
                    panic!("rpc result is not Ok")
                }
            } else {
                panic!("unexpected message type")
            }
        } else {
            panic!("unexpected message response")
        }
    }

    #[tokio::test]
    async fn test_reconnect() {
        tracing_subscriber::fmt::init();

        let test_handler = TestHandler {};

        let mut executor = BoxcarExecutor::new();
        executor.add_handler(Box::new(test_handler)).await;

        let server = Server::new(TcpListener::bind("127.0.0.1:9934").await.unwrap(), executor);

        tokio::task::spawn(async move { server.serve().await });

        // give the server a minute to start up
        sleep(Duration::from_secs(1)).await;

        let client = Client::new("ws://127.0.0.1:9934").await.unwrap();

        let command = RpcRequest {
            method: "foo".to_string(),
            body: vec![],
            subscribe: true,
        };
        let r = client.call(command).await;
        assert_eq!(r.is_ok(), true);

        let s_slot = r.unwrap();

        client.close().await;

        //
        // sleep(Duration::from_secs(5)).await;
        // println!("{:?}", result.try_recv().await)
    }
}