use anyhow::Context;
use futures::FutureExt;
use restate_sdk::prelude::{Endpoint, HttpServer};
use serde::{Deserialize, Serialize};
use testcontainers::core::wait::HttpWaitStrategy;
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    ContainerAsync, ContainerRequest, GenericImage, ImageExt,
};
use tokio::{io::AsyncBufReadExt, net::TcpListener, task};
use tracing::{error, info, warn};

// From restate-admin-rest-model
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

pub struct TestEnvironment {
    container_name: String,
    container_tag: String,
    logging: bool,
}

impl Default for TestEnvironment {
    fn default() -> Self {
        Self {
            container_name: "docker.io/restatedev/restate".to_string(),
            container_tag: "latest".to_string(),
            logging: false,
        }
    }
}

impl TestEnvironment {
    // --- Builder methods

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_container_logging(mut self) -> Self {
        self.logging = true;
        self
    }

    pub fn with_container(mut self, container_name: String, container_tag: String) -> Self {
        self.container_name = container_name;
        self.container_tag = container_tag;

        self
    }

    // --- Start method

    pub async fn start(self, endpoint: Endpoint) -> Result<StartedTestEnvironment, anyhow::Error> {
        let started_endpoint = StartedEndpoint::serve_endpoint(endpoint).await?;
        let started_restate_container = StartedRestateContainer::start_container(&self).await?;
        if let Err(e) = started_restate_container
            .register_endpoint(&started_endpoint)
            .await
        {
            return Err(anyhow::anyhow!("Failed to register endpoint: {e}"));
        }

        Ok(StartedTestEnvironment {
            _started_endpoint: started_endpoint,
            started_restate_container,
        })
    }
}

struct StartedEndpoint {
    port: u16,
    _cancel_tx: tokio::sync::oneshot::Sender<()>,
}

impl StartedEndpoint {
    async fn serve_endpoint(endpoint: Endpoint) -> Result<StartedEndpoint, anyhow::Error> {
        info!("Starting endpoint server...");

        // 0.0.0.0:0 will listen on a random port, both IPv4 and IPv6
        let host_address = "0.0.0.0:0".to_string();
        let listener = TcpListener::bind(host_address)
            .await
            .expect("listener can bind");
        let listening_addr = listener.local_addr()?;
        let endpoint_server_url =
            format!("http://{}:{}", listening_addr.ip(), listening_addr.port());

        // Start endpoint server
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            HttpServer::new(endpoint)
                .serve_with_cancel(listener, cancel_rx)
                .await;
        });

        let client = reqwest::Client::builder().http2_prior_knowledge().build()?;

        // wait for endpoint server to respond
        let mut retries = 0;
        loop {
            match client
                .get(format!("{endpoint_server_url}/health",))
                .send()
                .await
            {
                Ok(res) if res.status().is_success() => break,
                Ok(res) => {
                    warn!("Error when waiting for service endpoint server to be healthy, got response {}", res.status());
                    retries += 1;
                    if retries > 10 {
                        anyhow::bail!("Service endpoint server failed to start")
                    }
                }
                Err(err) => {
                    warn!("Error when waiting for service endpoint server to be healthy, got error {}", err);
                    retries += 1;
                    if retries > 10 {
                        anyhow::bail!("Service endpoint server failed to start")
                    }
                }
            }
        }

        info!("Service endpoint server listening at: {endpoint_server_url}",);

        Ok(StartedEndpoint {
            port: listening_addr.port(),
            _cancel_tx: cancel_tx,
        })
    }
}

struct StartedRestateContainer {
    _cancel_tx: tokio::sync::oneshot::Sender<()>,
    container: ContainerAsync<GenericImage>,
    ingress_url: String,
}

impl StartedRestateContainer {
    async fn start_container(
        test_environment: &TestEnvironment,
    ) -> Result<StartedRestateContainer, anyhow::Error> {
        let image = GenericImage::new(
            &test_environment.container_name,
            &test_environment.container_tag,
        )
        .with_exposed_port(8080.tcp())
        .with_exposed_port(9070.tcp())
        .with_wait_for(WaitFor::Http(
            HttpWaitStrategy::new("/restate/health")
                .with_port(8080.tcp())
                .with_response_matcher(|res| res.status().is_success()),
        ))
        .with_wait_for(WaitFor::Http(
            HttpWaitStrategy::new("/health")
                .with_port(9070.tcp())
                .with_response_matcher(|res| res.status().is_success()),
        ));

        // Start container
        let container = ContainerRequest::from(image)
            // have to expose entire host network because testcontainer-rs doesn't implement selective SSH port forward from host
            // see https://github.com/testcontainers/testcontainers-rs/issues/535
            .with_host(
                "host.docker.internal",
                testcontainers::core::Host::HostGateway,
            )
            .start()
            .await?;

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        if test_environment.logging {
            let container_stdout = container.stdout(true);
            let mut stdout_lines = container_stdout.lines();
            let container_stderr = container.stderr(true);
            let mut stderr_lines = container_stderr.lines();

            // Spawn a task to copy data from the AsyncBufRead to stdout
            task::spawn(async move {
                tokio::pin!(cancel_rx);
                loop {
                    tokio::select! {
                        Some(stdout_line) = stdout_lines.next_line().map(|res| res.transpose()) => {
                            match stdout_line {
                                Ok(line) => info!("{}", line),
                                Err(e) => {
                                    error!("Error reading stdout from container stream: {}", e);
                                    break;
                                }
                            }
                        },
                        Some(stderr_line) = stderr_lines.next_line().map(|res| res.transpose()) => {
                            match stderr_line {
                                Ok(line) => warn!("{}", line),
                                Err(e) => {
                                    error!("Error reading stderr from container stream: {}", e);
                                    break;
                                }
                            }
                        }
                        _ = &mut cancel_rx => {
                            break;
                        }
                    }
                }
            });
        }

        // Resolve ingress url
        let host = container.get_host().await?;
        let ports = container.ports().await?;
        let ingress_port = ports.map_to_host_port_ipv4(8080.tcp()).unwrap();
        let ingress_url = format!("http://{}:{}", host, ingress_port);

        info!("Restate container started, listening on requests at {ingress_url}");

        Ok(StartedRestateContainer {
            _cancel_tx: cancel_tx,
            container,
            ingress_url,
        })
    }

    async fn register_endpoint(&self, endpoint: &StartedEndpoint) -> Result<(), anyhow::Error> {
        let host = self.container.get_host().await?;
        let ports = self.container.ports().await?;
        let admin_port = ports.map_to_host_port_ipv4(9070.tcp()).unwrap();

        let client = reqwest::Client::builder().http2_prior_knowledge().build()?;

        let deployment_uri: String = format!("http://host.docker.internal:{}/", endpoint.port);
        let deployment_payload = RegisterDeploymentRequestHttp {
            uri: deployment_uri,
            additional_headers: None,
            use_http_11: false,
            force: false,
            dry_run: false,
        };

        let register_admin_url = format!("http://{}:{}/deployments", host, admin_port);

        let response = client
            .post(register_admin_url)
            .json(&deployment_payload)
            .send()
            .await
            .context("Error when trying to register the service endpoint")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Got non success status code when trying to register the service endpoint: {}",
                response.status()
            )
        }

        Ok(())
    }
}

pub struct StartedTestEnvironment {
    _started_endpoint: StartedEndpoint,
    started_restate_container: StartedRestateContainer,
}

impl StartedTestEnvironment {
    pub fn ingress_url(&self) -> String {
        self.started_restate_container.ingress_url.clone()
    }
}
