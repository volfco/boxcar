

    server slot.
    the server slot is the location where an rpc's state is stored.

    client slot.
    the client slot is the location where the rpc request is stored. it is sent to the server so the client knows where
    to put the incomming data


Call an RPC Method

    // create an rpc request
    client --> RpcReq  --> server
    // server returns RpcReqStatus::Accepted with slot number
    client <-- RpcReqStatus <-- server


    // on this connection, watch for a state change on slot N
    client --> Watch --> server
    // server returns operation result
    client <-- OpCode <-- server




1. client is asked to send message
2. client talks to Endpoint to send message
3. Endpoint allocates c_slot id and builds BoxcarMessageWrapper
4. Endpoint encodes to binary and sends it
5. Endpoint receives incoming message and decodes it
6. Endpoint checks if c_slot is registered
7. Endpoint puts message into proper c_slot
8. Endpoint reads


## TODO
- Handle cases where RPC comes in, and thread_pool is full. 
- Document Adding additional threads to handle blocking work
- Add Monitoring Methods to expose pending tasks
- ```    let mut runtime = tokio::runtime::Builder::new()
        .threaded_scheduler()
        .max_threads(20)
        .build().expect("Failed to build runtime.");```