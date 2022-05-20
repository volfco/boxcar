use boxcar_rpc::{Client, RpcRequest};

#[tokio::main]
async fn main() {
    let client = Client::new("ws://127.0.0.1:9930").await.unwrap();

    let body = "hello world".as_bytes().to_vec();

    // use the sleep method, which will delay for 5 seconds
    let command = RpcRequest {
        method: "sleep".to_string(),
        body,
        subscribe: true,
        resources: None,
    };

    // send the RpcRequest message to the server
    // a u16 is returned, which represents the request ID
    let slot = client.call(command).await.unwrap();

    // check if we have a response. this should be none because we're still sleeping
    let response = client.try_recv(slot).await.unwrap();
    println!("server returned: {:?}", response);

    // close the client. note- this is not needed. all clients subscribed to the slot get the result
    client.close().await;

    // create a new client
    let client2 = Client::new("ws://127.0.0.1:9930").await.unwrap();

    // subscribe to the slot. this tells the client to allocate a client side slot, and inform the
    // server that this client wants to get updates for this slot
    let _ = client2.subscribe(slot).await;

    // optional- refresh the slot. this asks the server to send the last recorded result for the
    // task. The task could be done at this point, or not.
    let _ = client2.refresh(slot).await;

    // now we can wait for the result
    let response = client2.recv_result(slot).await.unwrap();
    println!("server returned (via client2): {:?}", response);
}
