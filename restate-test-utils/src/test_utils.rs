
use restate_admin_rest_model::deployments::RegisterDeploymentRequest;
use testcontainers::{core::{IntoContainerPort, WaitFor}, runners::AsyncRunner, ContainerAsync, ContainerRequest, GenericImage, ImageExt};
use serde::{Serialize, Deserialize};
use restate_sdk::{discovery::Service, errors::HandlerError};
use std::time::Duration;

#[derive(Serialize, Deserialize, Debug)]
struct VersionResponse {
    version:String,
    min_admin_api_version:u32,
    max_admin_api_version:u32
}


pub struct TestContainer {
    container:ContainerAsync<GenericImage>
}

impl TestContainer {

    pub async fn new() -> Result<Self, HandlerError> {

        let image = GenericImage::new("docker.io/restatedev/restate", "latest")
            .with_exposed_port(9070.tcp())
            .with_exposed_port(8080.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Ingress HTTP listening"));

        // have to expose entire host network because testcontainer-rs doesn't implement selective SSH port forward from host
        // see https://github.com/testcontainers/testcontainers-rs/issues/535
        let container = ContainerRequest::from(image)
            .with_host("host.docker.internal" , testcontainers::core::Host::HostGateway)
            .start().await?;
            // .await?;

        let strout = container.stdout_to_vec().await.unwrap();
        println!("{}", String::from_utf8(strout).unwrap());

        let host = container.get_host().await?;
        let ports = container.ports().await?;

        let admin_port = ports.map_to_host_port_ipv4(9070.tcp()).unwrap();
        let ingress_port = ports.map_to_host_port_ipv4(8080.tcp()).unwrap();

        let admin_url = format!("http://{}:{}/version", host, admin_port);

        println!("container host:     {} admin port: {} ingress port: {}",
            nu_ansi_term::Style::new().bold().paint(host.to_string()),
            nu_ansi_term::Style::new().bold().paint(admin_port.to_string()),
            nu_ansi_term::Style::new().bold().paint(ingress_port.to_string()));

        println!("admin url:          {}",
             nu_ansi_term::Style::new().bold().paint(admin_url.as_str()));

        let body = reqwest::get(admin_url)
            .await?
            .json::<VersionResponse>()
            .await?;

        println!("body:\n{:?}", body);

        Ok(TestContainer {container})
    }

    pub async fn register(&self, server_port:u16) -> Result<(), HandlerError> {

        let host = self.container.get_host().await?;
        let ports = self.container.ports().await?;

        let admin_port = ports.map_to_host_port_ipv4(9070.tcp()).unwrap();
        let server_url = format!("http://localhost:{}", admin_port);

        println!("health check:       {}/health", server_url);

        let client = reqwest::Client::builder().http2_prior_knowledge().build().unwrap();

        // wait for server to respond
        while let Err(err) = client.get(format!("{}/health", server_url).to_string())
                                .header("accept", "application/vnd.restate.endpointmanifest.v1+json")
                                .send().await {
            tokio::time::sleep(Duration::from_secs(1)).await;

            println!("health check:       {:?}", err);
        }

        let ok = client.get(format!("{}/health", server_url).to_string())
                                .header("accept", "application/vnd.restate.endpointmanifest.v1+json")
                                .send().await;

        println!("health check:       {:?}", ok);

        println!("host started");

        let uri = http::Uri::builder()
            .scheme("http")
            .authority(format!("host.docker.internal:{}", server_port))
            .path_and_query("/")
            .build().unwrap();

        let register_url = format!("http://{}:{}/deployments", host, admin_port);
        println!("uri_str:            {:?}", uri);

        let register_payload = RegisterDeploymentRequest::Http { uri, additional_headers: None, use_http_11: false, force: false, dry_run: false }; //, additional_headers: (), use_http_11: (), force: (), dry_run: () }
        //let register_payload = format!("{{\"uri\": \"host.docker.internal:{}/\", \"force\": true}}", server_port);

        println!("register payload:   \n{:?}", register_payload);

        // wait for registration
        let registed = client.post(register_url.to_string())
                            .json(&register_payload)
                            .send().await;

        assert!(registed.is_ok());

        let response =registed.unwrap();
        assert!(response.status().is_success());

        println!("register response:  {}\n{}", nu_ansi_term::Style::new().bold().paint(response.status().as_u16().to_string()), response.text().await.unwrap());
        println!("{}", nu_ansi_term::Style::new().bold().paint("host registered"));

        return Ok(());
    }

    pub async fn delay(milliseconds:u64) {
        tokio::time::sleep(Duration::from_millis(milliseconds)).await;
    }

    pub async fn invoke(&self, service:Service, handler:String) -> Result<(), HandlerError> {

        let host = self.container.get_host().await?;
        let ports = self.container.ports().await?;

        let client = reqwest::Client::builder().http2_prior_knowledge().build().unwrap();

        let service_name:String = service.name.to_string();
        let handler_names:Vec<String> = service.handlers.iter().map(|h|h.name.to_string()).collect();

        assert!(handler_names.contains(&handler));

        let admin_port = ports.map_to_host_port_ipv4(9070.tcp()).unwrap();
        let admin_host = format!("http://{}:{}", host, admin_port);

        let service_discovery_url = format!("{}/services/{}/handlers", admin_host, service_name).to_string();

        // TODO query admin API to verify registered handlers matches service camelCase descriptor
        let discovery_response = client.get(service_discovery_url)
            .send().await;

        let resposne = discovery_response.unwrap();
        println!("discovery response:         {:?}", resposne);
        println!("discovery response:         {:?}", resposne.text().await.unwrap());

        let ingress_port = ports.map_to_host_port_ipv4(8080.tcp()).unwrap();
        let ingress_host = format!("http://{}:{}", host, ingress_port);

        let ingress_handler_url = format!("{}/{}/{}", ingress_host, service_name, handler).to_string();
        println!("ingress url:         {}", ingress_handler_url);

        let ingress_response = client.post(ingress_handler_url)
            .send().await;

        let resposne = ingress_response.unwrap();
        println!("ingress response:         {:?}", resposne);
        println!("ingress response:         {:?}", resposne.text().await.unwrap());

        return Ok(());

    }
}

#[tokio::test]
async fn boot_test_container() {
    let _test_comtainer = crate::test_utils::TestContainer::new().await.unwrap();
}
