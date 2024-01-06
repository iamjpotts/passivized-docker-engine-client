#![allow(unstable_name_collisions)]  // itertools::intersperse

#[cfg(not(target_os = "macos"))]  // On Macs, containers run in a VM, and their network is inaccessible.
#[cfg(not(windows))]  // No registry image available for Windows
#[path = "test_utils/lib.rs"]
mod test_utils;


// Pull from the docker.io registry, then push to a private registry
//
// Constraints:
//  * The client of the private registry will be the Docker server, not our client
//  * The private registry will be secured with Basic authentication to test authentication
//  * The docker registry software requires TLS when authentication is enabled
//  * The docker server will not recognize a fake host name or fake DNS name
//  * Docker typically auto-generates the IP address at runtime
//  * x509 certificates cannot use an IP address range
//
// See also https://docs.docker.com/registry/insecure/
#[tokio::test]
#[cfg(not(target_os = "macos"))]  // On Macs, containers run in a VM, and their network is inaccessible.
#[cfg(not(windows))]  // No registry image available for Windows
async fn test_push_to_authenticated_registry() {
    use std::io::Read;
    use std::net::Ipv4Addr;
    use std::path::PathBuf;

    use hyper::StatusCode;
    use hyper_tls::native_tls::{Certificate, Identity, TlsConnector};
    use itertools::Itertools;
    use log::info;
    use openssl::pkey::{PKey, Private};
    use tar::{Archive, Builder, Header};
    use tempfile::tempdir;

    use test_utils::images::{dind, web, registry};
    use test_utils::random_name;
    use test_utils::certs::{certificate_from_pem_file, extract_ip_address_from_cert_def};

    use passivized_docker_engine_client::DockerEngineClient;
    use passivized_docker_engine_client::errors::DecUseError;
    use passivized_docker_engine_client::model::{NetworkIpam, NetworkIpamConfig, RegistryAuth, Tar};
    use passivized_docker_engine_client::model::MountMode::ReadOnly;
    use passivized_docker_engine_client::requests::{BuildImageRequest, CreateContainerRequest, CreateNetworkRequest, EndpointConfig, HostConfig, NetworkingConfig};
    use passivized_docker_engine_client::responses::BuildImageResponseStreamItem;
    use passivized_htpasswd::Algo::BcryptMinCost;
    use passivized_htpasswd::Htpasswd;
    use passivized_test_support::http_status_tests::{equals, is_success};
    use passivized_test_support::logging;
    use passivized_test_support::waiter::{wait_for_https_server_with_backoff, wait_for_tcp_server_with_backoff};

    const HTPASSWD_USERNAME: &str = "foo";
    const HTPASSWD_PASSWORD: &str = "bar";

    const FN: &str = "test_push_to_authenticated_registry";

    logging::enable();

    let public = DockerEngineClient::new()
        .unwrap();

    public.images().pull_if_not_present(dind::IMAGE, dind::TAG)
        .await
        .unwrap();

    public.images().pull_if_not_present(registry::IMAGE, registry::TAG)
        .await
        .unwrap();

    const NETWORK_NAME: &str = "push_to_authenticated_network";

    let network_request = CreateNetworkRequest::default()
        .name(NETWORK_NAME)
        .ipam(NetworkIpam::default()
            .config(NetworkIpamConfig::default()
                // TODO: Detect an unused subnet
                .subnet("172.98.98.0/24")
                .gateway("172.98.98.1")
            )
        );

    let registry_ip = extract_ip_address_from_cert_def()
        .await;

    let network_create_response = public.networks().create(network_request)
        .await;

    match network_create_response {
        Ok(_) => {},

        // A prior run of the test created the network, but failed at a later step. Reuse the network.
        // However, if a container is still running from a failed test, an Address In Use error will
        // be generated when creating a container.
        Err(DecUseError::Rejected { status: StatusCode::CONFLICT, .. }) => {},

        // Test setup failed
        _ => {
            network_create_response.unwrap();
        }
    }

    let inspected_network = public.network(NETWORK_NAME).inspect()
        .await
        .unwrap();

    let network_containers: Vec<String> = inspected_network
        .containers
        .iter()
        .map(|(id, c)| c.
            name
            .as_ref()
            .map(|n| n.to_string())
            .unwrap_or(id.to_string())
        )
        .sorted()
        .collect();

    assert!(
        network_containers.is_empty(),
        "Network {} has running containers: {}",
        NETWORK_NAME,
        network_containers
            .into_iter()
            .sorted()
            .intersperse(", ".into())
            .collect::<String>()
    );

    let subnet = inspected_network
        .ipam
        .config
        .get(0)
        .unwrap()
        .subnet
        .as_ref()
        .unwrap();

    let mut parts = subnet.split("/");
    let block_addr = parts.next().unwrap().parse::<Ipv4Addr>().unwrap();
    let block_size = parts.next().unwrap().parse::<u16>().unwrap();

    info!("Subnet addr: {}", block_addr);
    info!("Subnet size: {}", block_size);

    let tmp = tempdir()
        .unwrap();

    let project_root = PathBuf::from(file!())
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let htpasswd = tmp
        .path()
        .join("htpasswd")
        .to_str()
        .unwrap()
        .to_string();

    let mut passwords = Htpasswd::new();
    passwords.set_with(BcryptMinCost, HTPASSWD_USERNAME, HTPASSWD_PASSWORD)
        .unwrap();

    tokio::fs::write(&htpasswd, passwords.to_string().as_bytes())
        .await
        .unwrap();

    let output = project_root
        .join("certificate")
        .join("output")
        .canonicalize()
        .unwrap();

    let ca_pem = output
        .join("ca.pem")
        .to_str()
        .unwrap()
        .to_string();

    let server_crt = output
        .join("testregistry.locallan.crt")
        .to_str()
        .unwrap()
        .to_string();

    let server_key = output
        .join("testregistry.locallan-key.pem")
        .to_str()
        .unwrap()
        .to_string();

    // Start a private registry

    let registry_create_request: CreateContainerRequest = CreateContainerRequest::default()
        .name(random_name(FN))
        .image(format!("{}:{}", registry::IMAGE, registry::TAG))
        .expose_port("443/tcp")
        .env("REGISTRY_AUTH=htpasswd")
        .env("REGISTRY_AUTH_HTPASSWD_PATH=/secrets/htpasswd")
        .env("REGISTRY_AUTH_HTPASSWD_REALM=Registry Realm")
        .env("REGISTRY_HTTP_ADDR=0.0.0.0:443")
        .env("REGISTRY_HTTP_TLS_CERTIFICATE=/secrets/server.crt")
        .env("REGISTRY_HTTP_TLS_KEY=/secrets/server.key")
        .host_config(HostConfig::default()
            .auto_remove()
            .mount(htpasswd, "/secrets/htpasswd", ReadOnly)
            .mount(&server_crt, "/secrets/server.crt", ReadOnly)
            .mount(server_key, "/secrets/server.key", ReadOnly)
            .network_mode(NETWORK_NAME)
        )
        .networking_config(
            NetworkingConfig::default()
                .endpoint(NETWORK_NAME, EndpointConfig::from(registry_ip))
        );

    let registry = public.containers().create(registry_create_request)
        .await
        .unwrap();

    public.container(&registry.id).start()
        .await
        .unwrap();

    let registry_url = format!("https://{registry_ip}");

    info!("registry url: {registry_url}");

    let registry_ca = certificate_from_pem_file(&ca_pem)
        .await;

    let registry_tls = TlsConnector::builder()
        .add_root_certificate(registry_ca)
        .build()
        .unwrap();

    wait_for_https_server_with_backoff(
        registry_url,
        registry_tls,
        is_success(),
        imp::build_backoff_for_registry()
    )
        .await
        .unwrap();

    // Since we cannot easily inject a temporary certificate authority into the host docker server,
    // spin up a docker-in-docker container and inject the CA into that instead. Then connect to
    // the DIND server using HTTPS, instead of the host docker server, and exercise the private
    // registry authenticated pull and push using DIND.

    let dind_create_request: CreateContainerRequest = CreateContainerRequest::default()
        .name(random_name("push_to_authenticated_dind"))
        .image(format!("{}:{}", dind::IMAGE, dind::TAG))
        .expose_port(format!("{}/tcp", dind::PORT))
        .host_config(HostConfig::default()
            .auto_remove()
            .privileged()
            .mount(ca_pem, format!("/etc/docker/certs.d/{}/ca.crt", registry_ip), ReadOnly)
            .network_mode(NETWORK_NAME)
        );

    let dind = public.containers().create(dind_create_request)
        .await
        .unwrap();

    public.container(&dind.id).start()
        .await
        .unwrap();

    let inspected_dind = public.container(&dind.id).inspect()
        .await
        .unwrap();

    let dind_ip = inspected_dind.first_ip_address()
        .unwrap();

    let dind_url = format!("https://{}:{}", dind_ip, dind::PORT);

    info!("dind url: {dind_url}");

    wait_for_tcp_server_with_backoff(&dind_ip, dind::PORT, imp::build_backoff_for_dind())
        .await
        .unwrap();

    let client_cert_tar = public.container(&dind.id).files().get("/certs/")
        .await
        .unwrap();

    let mut a = Archive::new(client_cert_tar.0.as_slice());

    let mut client_cert_pem_bytes: Option<Vec<u8>> = None;
    let mut client_key_pem_bytes: Option<Vec<u8>> = None;
    let mut ca_cert_pem_bytes: Option<Vec<u8>> = None;

    for file in a.entries().unwrap() {
        let mut f = file.unwrap();

        // Because .path() would be too easy, lol
        let p = f.header().path().unwrap().to_str().unwrap().to_string();

        match p.as_ref() {
            "certs/client/cert.pem" => {
                let mut content: Vec<u8> = Vec::new();
                f.read_to_end(&mut content).unwrap();

                client_cert_pem_bytes = Some(content);
            },
            "certs/client/key.pem" => {
                let mut content: Vec<u8> = Vec::new();
                f.read_to_end(&mut content).unwrap();

                client_key_pem_bytes = Some(content);
            },
            "certs/server/ca.pem" => {
                let mut content: Vec<u8> = Vec::new();
                f.read_to_end(&mut content).unwrap();

                ca_cert_pem_bytes = Some(content);
            }
            _ => {}
        }
    }

    let client_pem = client_cert_pem_bytes.unwrap();
    let client_key = client_key_pem_bytes.unwrap();

    let client_pk: PKey<Private> = PKey::private_key_from_pem(&client_key).unwrap();
    let client_pk8 = client_pk.private_key_to_pem_pkcs8().unwrap();

    let client_id = Identity::from_pkcs8(&client_pem, &client_pk8)
        .unwrap();

    let ca_pem = ca_cert_pem_bytes.unwrap();
    let ca = Certificate::from_pem(&ca_pem).unwrap();

    let tls = TlsConnector::builder()
        .identity(client_id)
        .add_root_certificate(ca)
        .build()
        .unwrap();

    wait_for_https_server_with_backoff(
        dind_url,
        tls.clone(),
        equals(StatusCode::NOT_FOUND),
        imp::build_backoff_for_dind()
    ).await.unwrap();

    let private_url = format!("https://{}:{}", dind_ip, dind::PORT);
    let private = DockerEngineClient::with_tls_config(&private_url, tls.clone())
        .unwrap();

    let private_images = private.images().list().await.unwrap();
    assert_eq!(0, private_images.len());

    // Attempt to pull an image into DIND that doesn't yet exist in the private registry.
    // The anonymous pull should result in an authentication failure, not a "not found" error.

    let private_image = format!("{registry_ip}/nginx");
    let private_tag = web::TAG;

    let pull_anonymous_failure = private.images().pull(&private_image, private_tag)
        .await
        .unwrap_err();

    if let DecUseError::Rejected { status: StatusCode::INTERNAL_SERVER_ERROR, message } = pull_anonymous_failure {
        assert!(message.contains("no basic auth credentials"));
    }
    else {
        panic!("Unexpected failure: {:?}", pull_anonymous_failure);
    }

    // Attempt to pull an image into DIND that doesn't yet exist in the private registry.
    // The authenticated pull should result in a "not found" error.

    let credential = RegistryAuth {
        username: HTPASSWD_USERNAME.into(),
        password: HTPASSWD_PASSWORD.into(),
        server: Some(registry_ip.to_string()),
        ..RegistryAuth::default()
    };

    let private_with_auth = DockerEngineClient::with_tls_config(private_url, tls)
        .unwrap()
        .with_registry_auth(credential);

    let pull_auth_failure = private_with_auth.images().pull(&private_image, private_tag)
        .await
        .unwrap_err();

    if let DecUseError::NotFound { message } = pull_auth_failure {
        let expected_message = format!("manifest for {}:{} not found: manifest unknown", private_image, private_tag);
        assert!(message.contains(&expected_message));
    }
    else {
        panic!("Unexpected failure: {:?}", pull_auth_failure);
    }

    // Pull a public image so we can tag it and push it to the authenticated private registry.
    // Authentication is not needed so we won't use it.

    private.images().pull_if_not_present(web::IMAGE, web::TAG)
        .await
        .unwrap();

    // Tag the pulled image so we can push it to the authenticated registry.
    private.images().tag(format!("{}:{}", web::IMAGE, web::TAG), &private_image, private_tag)
        .await
        .unwrap();

    // Push, using authentication
    private_with_auth.images().push(&private_image, private_tag)
        .await
        .unwrap();

    // Delete local tag
    private.images().untag(format!("{}:{}", private_image, private_tag))
        .await
        .unwrap();

    // Verify its gone
    let found = private.images().list()
        .await
        .unwrap()
        .iter()
        .any(|item| item
            .repo_tags
            .contains(&format!("{}:{}", &private_image, private_tag))
        );
    assert!(!found);

    // Pull, using authentication
    private_with_auth.images().pull(&private_image, private_tag)
        .await
        .unwrap();

    // Verify its back
    let found = private.images().list()
        .await
        .unwrap()
        .iter()
        .any(|item| item
            .repo_tags
            .contains(&format!("{}:{}", private_image, private_tag))
        );
    assert!(found);

    // Delete local tag again
    private.images().untag(format!("{}:{}", private_image, private_tag))
        .await
        .unwrap();

    // Build a new image using an image secured behind the registry
    {
        let mut builder = Builder::new(Vec::new());

        let docker_file_text = format!("FROM {}:{}", private_image, private_tag);
        let docker_file = docker_file_text.as_bytes();

        let mut header = Header::new_gnu();
        header.set_path("Dockerfile")
            .unwrap();
        header.set_size(docker_file.len() as u64);
        header.set_cksum();

        builder.append(&header, docker_file)
            .unwrap();

        let archive = Tar(builder.into_inner()
            .unwrap());

        let build_request = BuildImageRequest::default()
            .tag("secured:local-latest");

        // Expect an auth failure, because we didn't provide auth
        {
            let messages: Vec<BuildImageResponseStreamItem> = private
                .images()
                .build(build_request.clone(), archive.clone())
                .await
                .unwrap();

            let errors: Vec<&String> = messages
                .iter()
                .flat_map(|e| &e.error)
                .collect();

            assert!(errors
                .iter()
                .any(|e| e.contains("no basic auth credentials"))
            );
        }

        // Provide auth and expect success
        {
            let messages: Vec<BuildImageResponseStreamItem> = private_with_auth
                .images()
                .build(build_request, archive)
                .await
                .unwrap();

            let errors: Vec<&str> = messages
                .iter()
                .flat_map(|e| &e.error)
                .map(|e| e.as_str())
                .collect();

            assert_eq!(0, errors.len(), "{}", errors.join("\n"));
        }
    }

    public.container(registry.id).stop()
        .await
        .unwrap();

    public.container(dind.id).stop()
        .await
        .unwrap();

    imp::remove_network(&public, NETWORK_NAME)
        .await
        .unwrap();
}

#[cfg(not(target_os = "macos"))]  // On Macs, containers run in a VM, and their network is inaccessible.
#[cfg(not(windows))]  // No registry image available for Windows
mod imp {
    use backoff::backoff::Constant;
    use backoff::future::retry_notify;
    use log::warn;
    use passivized_test_support::retry::Limit;
    use passivized_docker_engine_client::DockerEngineClient;
    use passivized_docker_engine_client::errors::DecUseError;

    pub(super) fn build_backoff_for_dind() -> Limit {
        use std::time::Duration;

        let interval = Duration::from_secs(2);

        Limit::new(10, Constant::new(interval))
    }

    pub(super) fn build_backoff_for_registry() -> Limit {
        use std::time::Duration;

        let interval = Duration::from_secs(2);

        Limit::new(10, Constant::new(interval))
    }

    pub(super) fn build_backoff_for_network_removal() -> Limit {
        use std::time::Duration;

        let interval = Duration::from_secs(1);

        Limit::new(5, Constant::new(interval))
    }

    pub(super) async fn remove_network(docker: &DockerEngineClient, id: &str) -> Result<(), DecUseError> {
        retry_notify(
            build_backoff_for_network_removal(),
            || async {
                (match docker.network(id).remove().await {
                    Ok(()) => Ok(()),
                    Err(DecUseError::NotFound { .. }) => Ok(()),
                    Err(e) => Err(e)
                })
                    .map_err(backoff::Error::transient)
            },
            |error, _| warn!("Retrying after failure: {:?}", error)
        ).await
    }
}