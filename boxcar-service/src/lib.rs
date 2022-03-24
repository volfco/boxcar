// use boxcar_rpc;
// use etcd_rs;
// use std::collections::HashMap;
// use std::hash::Hash;
//
// /// BoxcarService takes an executor and BoxcarSever arguments. Wraps the Server and adds a custom
// /// handler. This handler exposes resource management
// struct BoxcarServiceBuilder {
//     etcd_client: etcd_rs::Client,
//     boxcar_server: boxcar_rpc::Server,
//     service_name: Option<None>,
//     weight: Option<u8>,
//     // TODO Make String something that can support other stuff
//     tags: HashMap<String, String>,
// }
// impl BoxcarServiceBuilder {
//     pub async fn new(etcd_client: etcd_rs::Client, boxcar_server: boxcar_rpc::Server) -> Self {
//         Self {
//             etcd_client,
//             boxcar_server,
//             service_name: None,
//             weight: None,
//             tags: Default::default(),
//         }
//     }
// }
//
// struct BoxcarService {}
// impl BoxcarService {
//     pub async fn new() -> Self {
//         todo!()
//     }
// }
//
// struct BoxcarServiceConnector {}
// impl BoxcarServiceConnector {
//     pub async fn new() -> Self {
//         todo!()
//     }
// }
