**under active development. no api stability until _0.2.0_!**

Boxcar is a suite of components that builds off of `boxcar-rpc`.

# boxcar-rpc
Boxcar RPC is an RPC framework that attempts to provide a fully asynchronous RPC interface.

Traditional frameworks, like gRPC, tie the execution to the connection between client/server. If the client disconnects 
and breaks the socket, you're out of luck for getting the result of the RPC.  While this is not a big issue (thank you TCP),
it still prevents the client from shutting down until the call is completed. 

Boxcar RPC is something that sits between your traditional RPC framework, and a traditional Task Queue. Celery/Sidekiq
are great tools for background processing, but are primarily task queues and require a broker (such as Redis) in order 
to work. 

Boxcar RPC natively uses a sever task queue internally for RPC execution. Any client can execute an RPC and can come back
later for the result (as long as the server has not restarted. Data is not persisted). The client supports both async
and sync execution- it's up to the user to select which they one.

Boxcar-rpc uses Websockets for communication.

# boxcar-cluster
Boxcar Cluster groups a bunch of boxcar-rpc servers into, well, a cluster. Uses etcd for discovery and metadata
