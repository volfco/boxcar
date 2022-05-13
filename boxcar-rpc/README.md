TBD




# State Diagram
```text

 + RpcReq               Client ---> Server  (req/rsp)
   ✓    RpcReqSlot      
   -    ServerError
   
 + RpcRslt              Server ---> Server  (streaming)
 
 + RpcReqRslt           Client ---> Server  (req/rsp)
   ✓    RpcRslt      
   -    ServerError
   
 + Sub                  Client ---> Server
   ✓    RpcRslt      
   -    ServerError
```