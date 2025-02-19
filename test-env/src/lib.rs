use restate_sdk::{
    errors::HandlerError,
    prelude::{Endpoint, HttpServer},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    ContainerAsync, ContainerRequest, GenericImage, ImageExt,
};
use tokio::{
    io::AsyncBufReadExt,
    net::TcpListener,
    task::{self, JoinHandle},
};
use tracing::{error, info, warn};

// addapted from from restate-admin-rest-model crate version 1.1.6
#[derive(Serialize, Deserialize, Debug)]
pub struct RegisterDeploymentRequestHttp {
    uri: String,
    additional_headers: Option<Vec<(String, String)>>,
    use_http_11: bool,
    force: bool,
    dry_run: bool,
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
    version: String,
    min_admin_api_version: u32,
    max_admin_api_version: u32,
}

pub struct TestContainerBuilder {
    container_name: String,
    container_tag: String,
    logging: bool,
}

impl Default for TestContainerBuilder {
    fn default() -> Self {
        TestContainerBuilder {
            container_name: "docker.io/restatedev/restate".to_string(),
            container_tag: "latest".to_string(),
            logging: false,
        }
    }
}

impl TestContainerBuilder {
    pub fn with_container_logging(mut self) -> TestContainerBuilder {
        self.logging = true;
        self
    }

    pub fn with_container(
        mut self,
        container_name: String,
        container_tag: String,
    ) -> TestContainerBuilder {
        self.container_name = container_name;
        self.container_tag = container_tag;

        self
    }

    pub fn build(self) -> TestContainer {
        TestContainer {
            builder: self,
            container: None,
            endpoint_task: None,
            endpoint_server_ip: None,
            endpoint_server_port: None,
            endpoint_server_url: None,
            ingress_url: None,
            stdout_logging: None,
            stderr_logging: None,
        }
    }
}

pub struct TestContainer {
    builder: TestContainerBuilder,
    container: Option<ContainerAsync<GenericImage>>,
    endpoint_task: Option<JoinHandle<()>>,
    endpoint_server_ip: Option<String>,
    endpoint_server_port: Option<u16>,
    endpoint_server_url: Option<String>,
    ingress_url: Option<String>,
    stdout_logging: Option<JoinHandle<()>>,
    stderr_logging: Option<JoinHandle<()>>,
}

impl Default for TestContainer {
    fn default() -> Self {
        TestContainerBuilder::default().build()
    }
}

impl TestContainer {
    pub fn builder() -> TestContainerBuilder {
        TestContainerBuilder::default()
    }

    pub async fn start(mut self, endpoint: Endpoint) -> Result<TestContainer, anyhow::Error> {
        self.serve_endpoint(endpoint).await?;
        self.start_container().await?;
        let registered = self.register_endpoint().await;
        if registered.is_err() {
            return Err(anyhow::anyhow!("Failed to register endpoint"));
        }

        Ok(self)
    }

    async fn serve_endpoint(&mut self, endpoint: Endpoint) -> Result<(), anyhow::Error> {
        info!("Starting endpoint server...");

        // use port 0 to allow tokio to assign unused port number
        let host_address = "127.0.0.1:0".to_string();
        let listener = TcpListener::bind(host_address)
            .await
            .expect("listener can bind");
        self.endpoint_server_ip = Some(listener.local_addr().unwrap().ip().to_string());
        self.endpoint_server_port = Some(listener.local_addr().unwrap().port());
        self.endpoint_server_url = Some(format!(
            "http://{}:{}",
            self.endpoint_server_ip.as_ref().unwrap(),
            self.endpoint_server_port.as_ref().unwrap()
        ));

        // boot restate server
        self.endpoint_task = Some(tokio::spawn(async move {
            HttpServer::new(endpoint).serve(listener).await;
        }));

        let client = reqwest::Client::builder().http2_prior_knowledge().build()?;

        // wait for server to respond
        let mut retries = 0;
        while client
            .get(format!(
                "{}/discover",
                self.endpoint_server_url.as_ref().unwrap()
            ))
            .header("accept", "application/vnd.restate.endpointmanifest.v1+json")
            .send()
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(100)).await;

            warn!("retrying endpoint server");

            retries += 1;
            if retries > 10 {
                return Err(anyhow::anyhow!("endpoint server failed to start"));
            }
        }

        info!(
            "endpoint server: {}",
            self.endpoint_server_url.as_ref().unwrap()
        );

        Ok(())
    }

    async fn start_container(&mut self) -> Result<(), anyhow::Error> {
        let image = GenericImage::new(&self.builder.container_name, &self.builder.container_tag)
            .with_exposed_port(9070.tcp())
            .with_exposed_port(8080.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Ingress HTTP listening"));

        // have to expose entire host network because testcontainer-rs doesn't implement selective SSH port forward from host
        // see https://github.com/testcontainers/testcontainers-rs/issues/535
        self.container = Some(
            ContainerRequest::from(image)
                .with_host(
                    "host.docker.internal",
                    testcontainers::core::Host::HostGateway,
                )
                .start()
                .await?,
        );

        if self.builder.logging {
            let container_stdout = self.container.as_ref().unwrap().stdout(true);
            let mut stdout_lines = container_stdout.lines();

            // Spawn a task to copy data from the AsyncBufRead to stdout
            let stdout_logging = task::spawn(async move {
                while let Some(line) = stdout_lines.next_line().await.transpose() {
                    match line {
                        Ok(line) => {
                            // Log each line using tracing
                            info!("{}", line);
                        }
                        Err(e) => {
                            // Log any errors
                            error!("Error reading from container stream: {}", e);
                            break;
                        }
                    }
                }
            });

            self.stderr_logging = Some(stdout_logging);

            let container_stderr = self.container.as_ref().unwrap().stderr(true);
            let mut stderr_lines = container_stderr.lines();

            // Spawn a task to copy data from the AsyncBufRead to stderr
            let stderr_logging = task::spawn(async move {
                while let Some(line) = stderr_lines.next_line().await.transpose() {
                    match line {
                        Ok(line) => {
                            // Log each line using tracing
                            error!("{}", line);
                        }
                        Err(e) => {
                            // Log any errors
                            error!("Error reading from container stream: {}", e);
                            break;
                        }
                    }
                }
            });

            self.stderr_logging = Some(stderr_logging);
        }

        let host = self.container.as_ref().unwrap().get_host().await?;
        let ports = self.container.as_ref().unwrap().ports().await?;

        let admin_port = ports.map_to_host_port_ipv4(9070.tcp()).unwrap();

        let admin_url = format!("http://{}:{}/version", host, admin_port);
        reqwest::get(admin_url)
            .await?
            .json::<VersionResponse>()
            .await?;

        Ok(())
    }

    async fn register_endpoint(&mut self) -> Result<(), HandlerError> {
        info!(
            "registering endpoint server: {}",
            self.endpoint_server_url.as_ref().unwrap()
        );

        let host = self.container.as_ref().unwrap().get_host().await?;
        let ports = self.container.as_ref().unwrap().ports().await?;

        let admin_port = ports.map_to_host_port_ipv4(9070.tcp()).unwrap();

        let client = reqwest::Client::builder().http2_prior_knowledge().build()?;

        let deployment_uri: String = format!(
            "http://host.docker.internal:{}/",
            self.endpoint_server_port.unwrap()
        );
        let deployment_payload = RegisterDeploymentRequestHttp {
            uri: deployment_uri,
            additional_headers: None,
            use_http_11: false,
            force: false,
            dry_run: false,
        }; //, additional_headers: (), use_http_11: (), force: (), dry_run: () }

        let register_admin_url = format!("http://{}:{}/deployments", host, admin_port);

        client
            .post(register_admin_url)
            .json(&deployment_payload)
            .send()
            .await?;

        let ingress_port = ports.map_to_host_port_ipv4(8080.tcp()).unwrap();
        self.ingress_url = Some(format!("http://{}:{}", host, ingress_port));

        info!("ingress url: {}", self.ingress_url.as_ref().unwrap());

        Ok(())
    }

    pub fn ingress_url(&self) -> String {
        self.ingress_url.clone().unwrap()
    }
}

impl Drop for TestContainer {
    fn drop(&mut self) {
        // testcontainers-rs already implements stop/rm on drop]
        // https://docs.rs/testcontainers/latest/testcontainers/

        // clean up tokio tasks
        if self.endpoint_task.is_some() {
            self.endpoint_task.take().unwrap().abort();
        }

        if self.stdout_logging.is_some() {
            self.stdout_logging.take().unwrap().abort();
        }

        if self.stderr_logging.is_some() {
            self.stderr_logging.take().unwrap().abort();
        }
    }
}
