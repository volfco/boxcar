**under active development. no api stability until _0.2.0_!**

Boxcar RPC is a framework that provides a fully asynchronous RPC interface. Or an attempt at once.

Think- one-to-one Celery/Sidekiq task queue. You define RPC Methods, and then clients can ask the server to execute a 
method, then close the connection and come back later for a result. Execution of the RPC is not bound to the lifetime of
the connection. 

Boxcar-rpc uses Websockets for communication.

You can think of Boxcar RPC as a distributed task queue, without the distributed part. If you want to add the 
distributed part back, [Railyard](https://github.com/volfco/railyard) is a weird hybrid of an ETL Pipeline and Distributed
Task Queue. 