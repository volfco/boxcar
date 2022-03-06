
// use prost_build::{Config, Service, ServiceGenerator};


// pub struct ServiceTraitGenerator {
//     mut_trait: bool,
// }
// impl ServiceGenerator for ServiceTraitGenerator {
//
//     fn generate(&mut self, service: Service, buf: &mut String) {
//
//         let service_name = quote::format_ident!("{}", service.name());
//         let server_trait = quote::format_ident!("{}", service.name());
//         let server_mod = quote::format_ident!("{}_server", service.name());
//
//
//         let mut trait_methods = String::new();
//         for method in &service.methods {
//             let mut line = format!(
//                 "        fn {}(&self, arguments: super::{}, ctx: &mut TaskContext) -> anyhow::Result<super::{}>;\n",
//                 method.name, method.input_type, method.output_type
//             );
//             if self.mut_trait {
//                 line = line.replace("&str", "&mut str");
//             }
//             trait_methods.push_str(&line);
//         }
//         let trait_methods = quote::format_ident!("{}", trait_methods);
//
//
//         quote! {
//
//             pub mod #service_name {
//
//                 pub trait Stub: Send + Sync + 'static {
//                     #trait_methods
//                 }
//
//                 pub struct Handler<T: Stub> {
//                     inner: std::sync::Arc<T>
//                 }
//                 impl<T: Stub> Handler<T> {
//                     pub fn new(inner: T) -> Box<Self> {
//                         Box::new(Self {
//                             inner: std::sync::Arc::new(inner)
//                         })
//                     }
//                 }
//
//
//
//                 /// Return a #service_name client
//                 pub async fn client(connection: bool) -> Result<(), ()> {
//
//                 }
//
//                 pub struct #service_nameClient {
//
//                 }
//
//             }
//
//             let client = DeviceManager::client(connection).await?;
//
//             let task = client.create(Message).delay().await?;
//
//             let task = client.create(Message).call().await?;
//         }
//
//     }
//
// }
//
// fn generate_router(methods: []) {
//     let mut method_match = quote::quote! { };
//     for method in methods {
//         method_match.extend(quote::quote! {
//             "SayHello" => {
//                 match OutputMessage::decode(&mut Cursor::new(body)) {
//                     Ok(message) => self.inner.say_hello(message).await,
//                     Err(error) => BoxcarError::DecodeError(error)
//                 }
//             }
//         });
//     }
//
//     quote! {
//         impl<T: Stub> BoxcarServerHandler for Handler<T> {
//             /// Pass the provided message body, `body`, to the method handler
//             pub fn call(&self, method: &str, body: &[u8]) -> Result<T, Error> {
//                 let mut buf = Vec::new();
//                 match method {
//                     #method_match
//                 }
//             }
//         }
//     }
// }
//
//
// fn discover_files(search: &str) -> Vec<PathBuf> {
//     let mut files = vec![];
//     for entry in WalkDir::new(search) {
//         let path = entry.unwrap().path().to_path_buf();
//         let ext = path.extension().and_then(OsStr::to_str);
//         if ext.is_some() && ext.unwrap() == "proto" {
//             files.push(path);
//         }
//     }
//     return files;
// }

fn main() {
    // let files = discover_files("src/proto");
    // Config::new()
    //     .protoc_arg("--experimental_allow_proto3_optional")
    //     // .service_generator(Box::new(ServiceTraitGenerator { mut_trait: false }))
    //     // .type_attribute(".", "#[derive(Serialize, Deserialize)]")
    //     .compile_protos(&*files, &[PathBuf::from_str("src/proto").unwrap()])
    //     .unwrap();
}
