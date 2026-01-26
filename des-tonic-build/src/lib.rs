use heck::{ToShoutySnakeCase, ToSnakeCase};
use std::error::Error;
use std::path::{Path, PathBuf};

pub struct Builder {
    out_dir: PathBuf,
    compile_well_known_types: bool,
}

impl Builder {
    pub fn new() -> Self {
        let out_dir = std::env::var_os("OUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));

        Self {
            out_dir,
            compile_well_known_types: false,
        }
    }

    pub fn out_dir(mut self, out: impl Into<PathBuf>) -> Self {
        self.out_dir = out.into();
        self
    }

    pub fn compile_well_known_types(mut self, yes: bool) -> Self {
        self.compile_well_known_types = yes;
        self
    }

    pub fn compile<P, I>(self, protos: &[P], includes: &[I]) -> Result<(), Box<dyn Error>>
    where
        P: AsRef<Path>,
        I: AsRef<Path>,
    {
        let mut config = prost_build::Config::new();
        config.out_dir(self.out_dir);
        if self.compile_well_known_types {
            config.compile_well_known_types();
        }

        config.service_generator(Box::new(DesTonicServiceGenerator));
        config.compile_protos(protos, includes)?;
        Ok(())
    }
}

struct DesTonicServiceGenerator;

impl prost_build::ServiceGenerator for DesTonicServiceGenerator {
    fn generate(&mut self, service: prost_build::Service, buf: &mut String) {
        let service_name = service.name.clone();
        let service_snake = service_name.to_snake_case();
        let service_proto_name = service.proto_name.clone();

        let fq_service = if service.package.is_empty() {
            service_proto_name.clone()
        } else {
            format!("{}.{}", service.package, service_proto_name)
        };

        for method in &service.methods {
            let const_name = format!("METHOD_{}", method.proto_name.to_shouty_snake_case());
            let path = format!("/{}/{}", fq_service, method.proto_name);
            buf.push_str(&format!("pub const {}: &str = \"{}\";\n", const_name, path));
        }

        buf.push_str(&format!("\npub mod {}_server {{\n", service_snake));
        buf.push_str("    use super::*;\n\n");
        buf.push_str("    #[::des_tonic::async_trait]\n");
        buf.push_str(&format!(
            "    pub trait {}: Send + Sync + 'static {{\n",
            service_name
        ));

        for method in &service.methods {
            let method_snake = method.name.to_snake_case();
            let req = &method.input_type;
            let resp = &method.output_type;

            let (req_ty, resp_ty) = match (method.client_streaming, method.server_streaming) {
                (false, false) => (
                    format!("::tonic::Request<{}>", req),
                    format!("::tonic::Response<{}>", resp),
                ),
                (false, true) => (
                    format!("::tonic::Request<{}>", req),
                    format!("::tonic::Response<::des_tonic::DesStreaming<{}>>", resp),
                ),
                (true, false) => (
                    format!("::tonic::Request<::des_tonic::DesStreaming<{}>>", req),
                    format!("::tonic::Response<{}>", resp),
                ),
                (true, true) => (
                    format!("::tonic::Request<::des_tonic::DesStreaming<{}>>", req),
                    format!("::tonic::Response<::des_tonic::DesStreaming<{}>>", resp),
                ),
            };

            buf.push_str(&format!(
                "        async fn {}(&self, request: {}) -> Result<{}, ::tonic::Status>;\n\n",
                method_snake, req_ty, resp_ty
            ));
        }

        buf.push_str("    }\n\n");
        buf.push_str(&format!(
            "    pub struct {}Server<T> {{\n        inner: ::std::sync::Arc<T>,\n    }}\n\n",
            service_name
        ));
        buf.push_str(&format!(
            "    impl<T: {}> {}Server<T> {{\n",
            service_name, service_name
        ));
        buf.push_str("        pub fn new(inner: T) -> Self {\n");
        buf.push_str("            Self { inner: ::std::sync::Arc::new(inner) }\n");
        buf.push_str("        }\n\n");
        buf.push_str("        pub fn register(self, router: &mut ::des_tonic::Router) {\n");
        buf.push_str("            router\n");

        for method in &service.methods {
            let method_snake = method.name.to_snake_case();
            let const_name = format!("METHOD_{}", method.proto_name.to_shouty_snake_case());
            let add = match (method.client_streaming, method.server_streaming) {
                (false, false) => "add_unary_prost",
                (false, true) => "add_server_streaming_prost",
                (true, false) => "add_client_streaming_prost",
                (true, true) => "add_bidi_streaming_prost",
            };

            buf.push_str(&format!(
                "                .{}({}, self.inner.clone(), |svc, req| async move {{\n                    svc.{}(req).await\n                }})\n",
                add, const_name, method_snake
            ));
        }

        buf.push_str("                ;\n");
        buf.push_str("        }\n");
        buf.push_str("    }\n");
        buf.push_str("}\n");

        buf.push_str(&format!("\npub mod {}_client {{\n", service_snake));
        buf.push_str("    use super::*;\n\n");
        buf.push_str("    #[derive(Clone)]\n");
        buf.push_str(&format!(
            "    pub struct {}Client {{\n        inner: ::des_tonic::Channel,\n        timeout: ::std::option::Option<::std::time::Duration>,\n    }}\n\n",
            service_name
        ));

        buf.push_str(&format!("    impl {}Client {{\n", service_name));
        buf.push_str(&format!(
            "        pub fn new(inner: ::des_tonic::Channel) -> Self {{\n            Self {{ inner, timeout: None }}\n        }}\n\n"
        ));
        buf.push_str(
            "        pub fn with_timeout(mut self, timeout: ::std::time::Duration) -> Self {\n            self.timeout = Some(timeout);\n            self\n        }\n\n",
        );

        for method in &service.methods {
            let method_snake = method.name.to_snake_case();
            let const_name = format!("METHOD_{}", method.proto_name.to_shouty_snake_case());
            let req = &method.input_type;
            let resp = &method.output_type;

            match (method.client_streaming, method.server_streaming) {
                (false, false) => {
                    buf.push_str(&format!(
                        "        pub async fn {}(&self, request: impl ::des_tonic::IntoRequest<{}>) -> Result<::tonic::Response<{}>, ::tonic::Status> {{\n",
                        method_snake, req, resp
                    ));
                    buf.push_str(&format!(
                        "            self.inner.unary_prost({}, request.into_request(), self.timeout).await\n",
                        const_name
                    ));
                    buf.push_str("        }\n\n");
                }
                (false, true) => {
                    buf.push_str(&format!(
                        "        pub async fn {}(&self, request: impl ::des_tonic::IntoRequest<{}>) -> Result<::tonic::Response<::des_tonic::DesStreaming<{}>>, ::tonic::Status> {{\n",
                        method_snake, req, resp
                    ));
                    buf.push_str(&format!(
                        "            self.inner.server_streaming_prost({}, request.into_request(), self.timeout).await\n",
                        const_name
                    ));
                    buf.push_str("        }\n\n");
                }
                (true, false) => {
                    buf.push_str(&format!(
                        "        pub async fn {}(&self) -> Result<(::des_tonic::stream::Sender<{}>, ::des_tonic::ClientResponseFuture<{}>), ::tonic::Status> {{\n",
                        method_snake, req, resp
                    ));
                    buf.push_str(&format!(
                        "            self.inner.client_streaming_prost({}, self.timeout).await\n",
                        const_name
                    ));
                    buf.push_str("        }\n\n");
                }
                (true, true) => {
                    buf.push_str(&format!(
                        "        pub async fn {}(&self) -> Result<(::des_tonic::stream::Sender<{}>, ::tonic::Response<::des_tonic::DesStreaming<{}>>), ::tonic::Status> {{\n",
                        method_snake, req, resp
                    ));
                    buf.push_str(&format!(
                        "            self.inner.bidirectional_streaming_prost({}, self.timeout).await\n",
                        const_name
                    ));
                    buf.push_str("        }\n\n");
                }
            }
        }

        buf.push_str("    }\n");
        buf.push_str("}\n");
    }

    fn finalize(&mut self, _buf: &mut String) {}
}
