# boxcar-cluster

`boxcar-cluster` allows you to cluster a bunch of boxcar servers together, and issue commands to the cluster. The task
will 


# architecture 
## etcd structure
```text
/<keyspace>/<service name>              Base Tree.
    /<uuid>                             Individual Service Entry
```