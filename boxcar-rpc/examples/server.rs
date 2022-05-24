use async_trait::async_trait;
use boxcar_rpc;
use boxcar_rpc::{BoxcarExecutor, BusWrapper, HandlerTrait, RpcResult, Server};
use std::time::Duration;
use tokio::time::sleep;

struct Example {}
#[async_trait]
impl HandlerTrait for Example {
    async fn call(&self, method: &str, arguments: Vec<u8>, _bus: BusWrapper) -> RpcResult {
        println!("example handler has been invoked. method: {}", method);

        if method == "sleep" {
            println!("sleeping for 5 seconds");
            sleep(Duration::from_secs(5)).await;
        }

        RpcResult::Ok(arguments)
    }

    fn contains(&self, _method: &str) -> bool {
        true
    }

    fn package(&self) -> &str {
        todo!()
    }
}

#[tokio::main]
async fn main() {
    let test_handler = Example {};

    let mut executor = BoxcarExecutor::new();
    executor.add_handler(Box::new(test_handler)).await;

    let server = Server::new()
        .bind("127.0.0.1:9930".parse().unwrap())
        .executor(executor);
    // start the server
    server.serve().await.unwrap()
}
