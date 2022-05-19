use boxcar_rpc::{Client, RpcRequest};

#[tokio::main]
async fn main() {
    let client = Client::new("ws://127.0.0.1:9930").await.unwrap();

    let body = "hello world".as_bytes().to_vec();

    let command = RpcRequest {
        method: "no_sleep".to_string(),
        body,
        subscribe: true,
        resources: None,
    };

    // send the RpcRequest message to the server
    // a u16 is returned, which represents the request ID
    let slot = client.call(command).await.unwrap();

    // wait for the response to be returned
    let response = client.recv(slot).await.unwrap();
    println!("server returned: {:?}", response);
}
