use nu_ansi_term::Style;
use reqwest::Response;
use testcontainers::{core::{IntoContainerPort, WaitFor}, runners::AsyncRunner, ContainerAsync, ContainerRequest, GenericImage, ImageExt};
use serde::{Serialize, Deserialize};
use restate_sdk::{discovery::Service, errors::HandlerError, prelude::{Endpoint, HttpServer}};
use tokio::{io::{self, AsyncWriteExt}, task::{self, JoinHandle}};
use std::time::Duration;

// addapted from from restate-admin-rest-model crate version 1.1.6
#[derive(Serialize, Deserialize, Debug)]
pub struct RegisterDeploymentRequestHttp {
    uri: String,
    additional_headers:Option<Vec<(String, String)>>,
    use_http_11: bool,
    force: bool,
    dry_run: bool
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RegisterDeploymentRequestLambda {
    arn: String,
    assume_role_arn: Option<String>,
    force: bool,
    dry_run: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct VersionResponse {
    version:String,
    min_admin_api_version:u32,
    max_admin_api_version:u32
}

pub struct TestContainer {
    container:ContainerAsync<GenericImage>,
    stdout_logging:JoinHandle<()>,
    stderr_logging:JoinHandle<()>,
    endpoint:Option<JoinHandle<()>>
}


impl TestContainer {

    //"docker.io/restatedev/restate", "latest"
    pub async fn new(image:String, version:String) -> Result<Self, HandlerError> {

        let image = GenericImage::new(image, version)
            .with_exposed_port(9070.tcp())
            .with_exposed_port(8080.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Ingress HTTP listening"));

        // have to expose entire host network because testcontainer-rs doesn't implement selective SSH port forward from host
        // see https://github.com/testcontainers/testcontainers-rs/issues/535
        let container = ContainerRequest::from(image)
            .with_host("host.docker.internal" , testcontainers::core::Host::HostGateway)
            .start()
            .await?;

        let mut container_stdout = container.stdout(true);
        // Spawn a task to copy data from the AsyncBufRead to stdout
        let stdout_logging = task::spawn(async move {
            let mut stdout = io::stdout();
            if let Err(e) = io::copy(&mut container_stdout, &mut stdout).await {
                eprintln!("Error copying data: {}", e);
            }
        });

        let mut container_stderr = container.stderr(true);
        // Spawn a task to copy data from the AsyncBufRead to stderr
        let stderr_logging = task::spawn(async move {
            let mut stderr = io::stderr();
            if let Err(e) = io::copy(&mut container_stderr, &mut stderr).await {
                eprintln!("Error copying data: {}", e);
            }
        });

        let host = container.get_host().await?;
        let ports = container.ports().await?;

        let admin_port = ports.map_to_host_port_ipv4(9070.tcp()).unwrap();

        let admin_url = format!("http://{}:{}/version", host, admin_port);
        reqwest::get(admin_url)
            .await?
            .json::<VersionResponse>()
            .await?;

        Ok(TestContainer {container, stdout_logging, stderr_logging, endpoint:None})
    }

    pub async fn serve_endpoint(&mut self, endpoint:Endpoint) {

        println!("\n\n{}\n\n", Style::new().bold().paint(format!("starting enpoint server...")));
        // uses higher port number to avoid collisions
        // with non-test instances running locally
        let host_port:u16 = 19080;
        let host_address = format!("0.0.0.0:{}", host_port);

        // boot restate server
        let endpoint = tokio::spawn(async move {
            HttpServer::new(endpoint)
                .listen_and_serve(host_address.parse().unwrap()).await;
        });

        let registered = self.register(host_port).await;

        assert!(registered.is_ok());

        self.endpoint = Some(endpoint);
    }

    async fn register(&self, server_port:u16) -> Result<(), HandlerError> {

        println!("\n\n{}\n\n", Style::new().bold().paint(format!("registering server...")));

        let host = self.container.get_host().await?;
        let ports = self.container.ports().await?;

        let admin_port = ports.map_to_host_port_ipv4(9070.tcp()).unwrap();
        let server_url = format!("http://localhost:{}", admin_port);

        let client = reqwest::Client::builder().http2_prior_knowledge().build()?;

        // wait for server to respond
        while let Err(_) = client.get(format!("{}/health", server_url).to_string())
                                .header("accept", "application/vnd.restate.endpointmanifest.v1+json")
                                .send().await {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        client.get(format!("{}/health", server_url).to_string())
            .header("accept", "application/vnd.restate.endpointmanifest.v1+json")
            .send().await?;

        let deployment_uri:String = format!("http://host.docker.internal:{}/", server_port);
        let deployment_payload = RegisterDeploymentRequestHttp {
                                    uri:deployment_uri,
                                    additional_headers:None,
                                    use_http_11: false,
                                    force: false,
                                    dry_run: false }; //, additional_headers: (), use_http_11: (), force: (), dry_run: () }

        let register_admin_url = format!("http://{}:{}/deployments", host, admin_port);

        client.post(register_admin_url.to_string())
            .json(&deployment_payload)
            .send().await?;

        let ingress_port = ports.map_to_host_port_ipv4(8080.tcp()).unwrap();
        let ingress_host = format!("http://{}:{}", host, ingress_port);

        println!("\n\n{}\n\n", Style::new().bold().paint(format!("ingress url: {}", ingress_host, )));

        return Ok(());
    }

    pub async fn delay(milliseconds:u64) {
        tokio::time::sleep(Duration::from_millis(milliseconds)).await;
    }

    pub async fn invoke(&self, service:Service, handler:String) -> Result<Response, HandlerError> {

        let host = self.container.get_host().await?;
        let ports = self.container.ports().await?;

        let client = reqwest::Client::builder().http2_prior_knowledge().build().unwrap();

        let service_name:String = service.name.to_string();
        let handler_names:Vec<String> = service.handlers.iter().map(|h|h.name.to_string()).collect();

        assert!(handler_names.contains(&handler));

        println!("\n\n{}\n\n", Style::new().bold().paint(format!("invoking {}/{}", service_name, handler)));

        let admin_port = ports.map_to_host_port_ipv4(9070.tcp()).unwrap();
        let admin_host = format!("http://{}:{}", host, admin_port);

        let service_discovery_url = format!("{}/services/{}/handlers", admin_host, service_name).to_string();

        client.get(service_discovery_url)
            .send().await?;

        // todo verify discovery response contains service/handler

        let ingress_port = ports.map_to_host_port_ipv4(8080.tcp()).unwrap();
        let ingress_host = format!("http://{}:{}", host, ingress_port);

        let ingress_handler_url = format!("{}/{}/{}", ingress_host, service_name, handler).to_string();

        let ingress_resopnse = client.post(ingress_handler_url)
            .send().await?;

        return Ok(ingress_resopnse);
    }
}

impl Drop for TestContainer {
    fn drop(&mut self) {

      // todo cleanup on drop?
      // testcontainers-rs already implements stop/rm on drop]
      // https://docs.rs/testcontainers/latest/testcontainers/
      //

    }
}

// #[tokio::test]
// async fn boot_test_container() {
//     let _test_comtainer = crate::test_utils::TestContainer::new("docker.io/restatedev/restate".to_string(), "latest".to_string()).await.unwrap();
// }
