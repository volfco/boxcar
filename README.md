Boxcar RPC is a framework that provides a fully asynchronous RPC interface. 

Boxcar-rpc uses Websockets for communication.

The client can initiate an RPC, disconnect, and then re-connect later on to retrieve the result. This is a departure from
existing frameworks, such a gRPC, where if the client disconnects the execution is lost. When using a more traditional 
framework, long-running calls can become an issue. The usual solution is to use a distributed job queue like bastion, 
sidekiq, or celery.

You can think of Boxcar RPC as a distributed task queue, without the distributed part. If you want to add the 
distributed part back, [Railyard](https://github.com/volfco/railyard) is a weird hybrid of an ETL Pipeline and Distributed
Task Queue. 