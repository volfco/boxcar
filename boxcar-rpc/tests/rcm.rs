extern crate core;

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use boxcar_rpc::rcm::ResourceManager;
    use boxcar_rpc::{
        BoxcarExecutor, BoxcarMessage, BusWrapper, HandlerTrait, RpcRequest, RpcResult,
    };
    use boxcar_rpc::{Client, Server};
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::time::sleep;

    struct TestHandler {}
    #[async_trait]
    impl HandlerTrait for TestHandler {
        async fn call(&self, _method: &str, _arguments: Vec<u8>, bus: BusWrapper) -> RpcResult {
            println!("{:?}", bus);
            tracing::info!("handler has started");
            sleep(Duration::from_secs(3)).await;
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
    async fn test_rcm_valid_allocation() {
        tracing_subscriber::fmt::init();

        let test_handler = TestHandler {};

        let mut executor = BoxcarExecutor::new();
        executor.add_handler(Box::new(test_handler)).await;

        let rcm = ResourceManager::new();
        rcm.add("test1", 1024);
        rcm.add("test2", 512);

        let server = Server::new(TcpListener::bind("127.0.0.1:9933").await.unwrap(), executor)
            .attach_rcm(rcm.clone());

        tokio::task::spawn(async move { server.serve().await });

        // give the server a minute to start up
        sleep(Duration::from_secs(1)).await;

        let client = Client::new("ws://127.0.0.1:9933").await.unwrap();

        let command = RpcRequest {
            method: "foo".to_string(),
            body: vec![],
            subscribe: true,
            resources: {
                let mut resource_map = HashMap::new();
                resource_map.insert("test1".to_string(), 512);

                Some(resource_map)
            },
        };
        let r = client.call(command).await;
        assert_eq!(r.is_ok(), true);

        let result = r.unwrap();

        // check if the resources have been consumed
        // we can directly access the Resource Manager as it's an Arc internally
        let rcmp = rcm.peak();
        assert_eq!(rcmp.get("test1"), Some(&512));

        sleep(Duration::from_secs(1)).await;

        let resp = client.recv(result).await;
        if let Ok(inner) = resp {
            if let BoxcarMessage::RpcRslt(inner) = inner {
                if let Some(RpcResult::Ok(buf)) = inner.1 {
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
}
