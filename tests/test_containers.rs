#[path = "../examples/example_utils/lib.rs"]
mod example_utils;

#[path = "test_utils/lib.rs"]
mod test_utils;

use std::collections::HashMap;
use std::time::Duration;

use const_str::concat;
use hyper::StatusCode;

#[cfg(not(target_os = "macos"))]
use log::{info, warn};

use test_utils::images::{EXPECTED_PLATFORM, hello, web};
use test_utils::random_name;

use passivized_docker_engine_client::DockerEngineClient;
use passivized_docker_engine_client::errors::DecUseError;
use passivized_docker_engine_client::model::{StreamKind, Unit};
use passivized_docker_engine_client::requests::{CreateContainerRequest, Filters, HostConfig, InspectContainerArgs, ListContainersRequest};

#[cfg(not(target_os = "macos"))]
use passivized_docker_engine_client::responses::FileSystemChangeKind;

use passivized_docker_engine_client::responses::Network;

#[cfg(not(target_os = "macos"))]
use passivized_test_support::logging;

#[cfg(not(target_os = "macos"))]
use passivized_test_support::http_status_tests::is_success;

#[cfg(not(target_os = "macos"))]
use passivized_test_support::waiter::wait_for_http_server;

#[cfg(not(windows))]
use passivized_docker_engine_client::requests::WaitCondition;

#[tokio::test]
async fn test_find_containers_by_label() {
    const FN: &str = "test_find_containers_by_label";

    let label_key = FN;
    let label_value1 = "whatever1";
    let label_value2 = "whatever2";

    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present(web::IMAGE, web::TAG)
        .await
        .unwrap();

    let request1: CreateContainerRequest = CreateContainerRequest::default()
        .name(random_name(concat!(FN, "_1")))
        .image(format!("{}:{}", web::IMAGE, web::TAG))
        .host_config(HostConfig::default()
            .auto_remove()
        )
        .label(label_key, label_value1);

    let request2: CreateContainerRequest = CreateContainerRequest::default()
        .name(random_name(concat!(FN, "_2")))
        .image(format!("{}:{}", web::IMAGE, web::TAG))
        .host_config(HostConfig::default()
            .auto_remove()
        )
        .label(label_key, label_value2);

    let container1 = dec.containers().create(request1)
        .await
        .unwrap();

    let container2 = dec.containers().create(request2)
        .await
        .unwrap();

    {
        // Get all with a label, regardless of label value.

        let list_request = ListContainersRequest::default()
            .all(true) // To get stopped containers
            .filters(
                Filters::default()
                    .label_present(label_key)
            );

        let list_response = dec.containers().list(list_request)
            .await
            .unwrap();

        assert_eq!(2, list_response.len());

        assert!(list_response.iter().find(|item| item.id == container1.id).is_some());
        assert!(list_response.iter().find(|item| item.id == container2.id).is_some());
    }

    {
        // Get containers with a specific label value

        let list_request = ListContainersRequest::default()
            .all(true) // To get stopped containers
            .filters(
                Filters::default()
                    .label_value(label_key, label_value2)
            );

        let list_response = dec.containers().list(list_request)
            .await
            .unwrap();

        assert_eq!(1, list_response.len());

        assert!(list_response.iter().find(|item| item.id == container2.id).is_some());
    }

    dec.container(container1.id).remove()
        .await
        .unwrap();

    dec.container(container2.id).remove()
        .await
        .unwrap();
}

#[tokio::test]
#[cfg(not(target_os = "macos"))]  // On Macs, containers run in a VM, and their network is inaccessible.
async fn test_get_changed_file_list() {
    const FN: &str = "test_get_changed_file_list";

    logging::enable();

    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present(web::IMAGE, web::TAG)
        .await
        .unwrap();

    let request: CreateContainerRequest = CreateContainerRequest::default()
        .name(random_name(FN))
        .image(format!("{}:{}", web::IMAGE, web::TAG));

    let container = dec.containers().create(request)
        .await
        .unwrap();

    dec.container(&container.id).start()
        .await
        .unwrap();

    let inspected = dec.container(&container.id).inspect()
        .await
        .unwrap();

    let ip = inspected.first_ip_address()
        .unwrap();

    if let Err(e) = wait_for_http_server(format!("http://{}", ip), is_success()).await {
        info!("Getting console output of failed container");

        let lines = String::from_iter(
            dec
                .container(&container.id)
                .logs()
                .await
                .unwrap()
                .iter()
                .map(|l| -> &str {l.text.as_ref()})
        );

        warn!("Log:\n{}", lines);

        panic!("{:?}", e);
    }

    // Because getting changed files on a running container isn't supported on Windows, stop the container now.
    dec.container(&container.id).stop()
        .await
        .unwrap();

    let changes = dec.container(&container.id).files().changes()
        .await
        .unwrap();

    println!("Changes: {:?}", changes);

    let found = changes
        .iter()
        .find(|item| item.path() == web::EXPECTED_ADDED_FILE.to_string())
        .unwrap();

    #[cfg(windows)]
    const EXPECTED_KIND: FileSystemChangeKind = FileSystemChangeKind::Modified;

    #[cfg(not(windows))]
    const EXPECTED_KIND: FileSystemChangeKind = FileSystemChangeKind::Added;

    assert_eq!(EXPECTED_KIND, found.kind());

    dec.container(&container.id).stop()
        .await
        .unwrap();

    dec.container(container.id).remove()
        .await
        .unwrap();
}

#[tokio::test]
async fn test_get_stdout_log() {
    const FN: &str = "test_get_stdout_log";

    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present(hello::IMAGE, hello::TAG)
        .await
        .unwrap();

    let request: CreateContainerRequest = CreateContainerRequest::default()
        .name(random_name(FN))
        .image(format!("{}:{}", hello::IMAGE, hello::TAG));

    let container = dec.containers().create(request)
        .await
        .unwrap();

    dec.container(&container.id).start()
        .await
        .unwrap();

    #[cfg(windows)]
    {
        let mut running = true;

        for _ in 0..30 {
            let inspected = dec.container(&container.id).inspect()
                .await
                .unwrap();

            running = inspected.state.running;

            if running {
                println!("Sleeping for 1 sec while still running");
                tokio::time::sleep(Duration::from_secs(1))
                    .await;
            }
            else {
                println!("State is {}", inspected.state.status);
                break;
            }
        }

        assert!(!running);
    }

    #[cfg(not(windows))]
    dec.container(&container.id).wait(WaitCondition::NotRunning)
        .await
        .unwrap();

    let log_lines = dec.container(&container.id).logs()
        .await
        .unwrap();

    dec.container(container.id).remove()
        .await
        .unwrap();

    assert!(log_lines.iter().all(|line| line.kind == StreamKind::StdOut));
    assert!(log_lines.iter().any(|line| line.text.contains("Hello from Docker!")));
}

#[tokio::test]
async fn test_get_top_processes() {
    const FN: &str = "test_get_top_processes";

    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present(web::IMAGE, web::TAG)
        .await
        .unwrap();

    let create_request = CreateContainerRequest::default()
        .name(random_name(FN))
        .image(format!("{}:{}", web::IMAGE, web::TAG))
        .host_config(HostConfig::default()
            .auto_remove()
        );

    let container = dec.containers().create(create_request)
        .await
        .unwrap();

    dec.container(&container.id).start()
        .await
        .unwrap();

    let top = dec.container(&container.id).top()
        .await
        .unwrap();

    println!("Titles: {:?}", top.titles);
    println!("Processes: {:?}", top.processes);

    #[cfg(target_os = "macos")]
    fn is_matching_heading(heading: &str) -> bool {
        heading == web::EXPECTED_PROCESS_HEADING || heading == "COMMAND"
    }

    #[cfg(not(target_os = "macos"))]
    fn is_matching_heading(heading: &str) -> bool {
        heading == web::EXPECTED_PROCESS_HEADING
    }

    let (index, _) = top
        .titles
        .iter()
        .enumerate()
        .find(|(_, item)| is_matching_heading(item))
        .unwrap();

    let cmd_found = top
        .processes
        .iter()
        .any(|p| p[index].contains(web::EXPECTED_PROCESS));

    assert!(cmd_found, "Found {} in {:?}", web::EXPECTED_PROCESS, top.processes);

    dec.container(&container.id).stop()
        .await
        .unwrap();
}

// TODO: Create a container using every available field, then inspect it, and verify the requested/created values are reflected back by inspection.
#[tokio::test]
async fn test_inspect_non_started_container_fields() {
    const FN: &str = "test_inspect_non_started_container_fields";

    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present(web::IMAGE, web::TAG)
        .await
        .unwrap();

    let name = random_name(FN);

    let request: CreateContainerRequest = CreateContainerRequest::default()
        .name(&name)
        .image(format!("{}:{}", web::IMAGE, web::TAG))
        .host_config(HostConfig::default()
            .network_mode("bridge")
        );

    let container = dec.containers().create(request.clone())
        .await
        .unwrap();

    assert_eq!(0, container.warnings.len());

    let inspected = dec.container(&container.id)
        .inspect_with(InspectContainerArgs::default().size(true))
        .await
        .unwrap();

    dec.container(&container.id).remove()
        .await
        .unwrap();

    let request_host_config = request.host_config
        .unwrap();

    // Compare fields. Assertions are in declaration order according to the Rust struct they are in.

    assert_eq!(container.id, inspected.id);
    assert!(inspected.created.contains("T"));
    assert_eq!(web::PATH, inspected.path);
    assert_eq!(web::args(), inspected.args);

    // State
    assert_eq!("created", inspected.state.status);
    assert!(!inspected.state.running);
    assert!(!inspected.state.paused);
    assert!(!inspected.state.restarting);
    assert!(!inspected.state.oom_killed);
    assert!(!inspected.state.dead);
    assert_eq!(0, inspected.state.pid);
    assert_eq!(0, inspected.state.exit_code);
    assert_eq!("0001-01-01T00:00:00Z", inspected.state.started_at);
    assert_eq!("0001-01-01T00:00:00Z", inspected.state.finished_at);
    assert!(inspected.state.health.is_none());

    assert_eq!(web::HASH, inspected.image);
    assert_eq!("", inspected.resolv_conf_path);
    assert_eq!("", inspected.hostname_path);
    assert_eq!("", inspected.hosts_path);
    assert_eq!("", inspected.log_path);
    assert_eq!(format!("/{}", name), inspected.name);
    assert_eq!(0, inspected.restart_count);
    assert!(web::expected_driver().contains(&inspected.driver), "Should contain one of {:?} but was {}", web::expected_driver(), inspected.driver);
    assert_eq!(EXPECTED_PLATFORM, inspected.platform);
    assert_eq!("", inspected.mount_label);
    assert_eq!("", inspected.process_label);
    assert_eq!("", inspected.app_armor_profile);
    assert_eq!(0, inspected.exec_ids.len());

    assert_eq!(request_host_config.network_mode.unwrap(), inspected.host_config.network_mode);
    assert_eq!(request_host_config.privileged, inspected.host_config.privileged);

    assert!(web::expected_driver().contains(&inspected.graph_driver.name));

    if inspected.graph_driver.name == "btrfs" {
        // Fedora uses btrfs with no parameters
        assert_eq!(0, inspected.graph_driver.data.len());
    }
    else {
        #[cfg(windows)]
        const EXPECTED_DATA_KEY: &str = "dir";

        #[cfg(not(windows))]
        const EXPECTED_DATA_KEY: &str = "LowerDir";

        assert!(inspected.graph_driver.data.contains_key(EXPECTED_DATA_KEY));
    }

    // Windows does not report the size
    #[cfg(windows)]
    assert_eq!(0, inspected.size_root_fs.unwrap());

    #[cfg(not(windows))]
    assert!(
        inspected.size_root_fs.unwrap() > web::MIN_SIZE,
        "Expected size_root_fs to be larger than {} but was {}",
        web::MIN_SIZE,
        inspected.size_root_fs.unwrap()
    );

    assert_eq!(Some(0), inspected.size_rw);

    assert_eq!(0, inspected.mounts.len());

    // Config
    assert!(inspected.config.hostname.is_some());
    assert_eq!("", inspected.config.domain_name);
    assert_eq!("", inspected.config.user);
    assert_eq!(false, inspected.config.attach_stdin);
    assert_eq!(false, inspected.config.attach_stdout);
    assert_eq!(false, inspected.config.attach_stderr);
    assert_eq!(
        HashMap::from_iter(
            web::exposed_ports()
                .iter()
                .map(|ep| (ep.clone(), Unit {}))
        ),
        inspected.config.exposed_ports
    );
    assert_eq!(false, inspected.config.tty);
    assert_eq!(false, inspected.config.open_stdin);
    assert_eq!(false, inspected.config.stdin_once);
    assert!(
        inspected.config.env.contains(&web::EXPECTED_ENV.to_string()),
        "Expected to find {} in env but was {:?}",
        web::EXPECTED_ENV,
        inspected.config.env
    );
    assert_eq!(web::cmd(), inspected.config.cmd);
    assert_eq!(None, inspected.config.health_check);
    assert_eq!(concat!(web::IMAGE, ":", web::TAG), inspected.config.image.unwrap());
    assert_eq!(0, inspected.config.volumes.len());
    assert_eq!(web::WORKING_DIR, inspected.config.working_dir);
    assert_eq!(web::entrypoint(), inspected.config.entry_point);
    assert_eq!(None, inspected.config.network_disabled);
    assert_eq!(None, inspected.config.mac_address);
    assert_eq!(0, inspected.config.on_build.len());
    assert_eq!(
        web::LABEL_VALUE
            .to_string(),
        inspected.config.labels.get(web::LABEL_KEY)
            .unwrap()
            .to_string()
    );
    assert_eq!(None, inspected.config.stop_signal);
    assert_eq!(None, inspected.config.stop_timeout_seconds);
    assert_eq!(0, inspected.config.shell.len());

    // Bridge aliases are set on Windows but not on Mac or Linux

    #[cfg(windows)]
    let expected_aliases = inspected.network_settings.networks
        .get("bridge")
        .expect("bridge network")
        .aliases
        .as_ref()
        .expect("bridge network aliases")
        .clone()
        .into();

    #[cfg(not(windows))]
    let expected_aliases = None;

    // Network settings
    assert_eq!(HashMap::new(), inspected.network_settings.ports);
    assert_eq!(
        HashMap::from([(
            "bridge".to_string(),
            Network {
                ipam_config: None,
                links: None,
                aliases: expected_aliases,
                network_id: "".into(),
                endpoint_id: "".into(),
                gateway: "".into(),
                ip_address: "".into(),
                ip_prefix_len: 0,
                ipv6_gateway: "".into(),
                global_ipv6_address: "".into(),
                global_ipv6_prefix_len: 0,
                mac_address: "".into(),
                driver_opts: None
            }
        )]),
        inspected.network_settings.networks
    );
}

#[tokio::test]
async fn test_remove_running_container() {
    const FN: &str = "test_remove_running_container";

    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present(web::IMAGE, web::TAG)
        .await
        .unwrap();

    let name = random_name(FN);

    let request: CreateContainerRequest = CreateContainerRequest::default()
        .name(&name)
        .image(format!("{}:{}", web::IMAGE, web::TAG))
        .host_config(HostConfig::default()
            .auto_remove()
        );

    let container = dec.containers().create(request)
        .await
        .unwrap();

    dec.container(&container.id).start()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(1))
        .await;

    let removal_error = dec.container(&container.id).remove()
        .await
        .unwrap_err();

    if let DecUseError::Rejected { status, message } = removal_error {
        assert_eq!(StatusCode::CONFLICT, status);
        assert!(
            message.contains("cannot remove a running container")
            ||
            message.contains("container is running: stop the container before removing or force remove"),
            "message was: {message}"
        );
    }
    else {
        panic!("Did not expect failure {}", removal_error);
    }

    dec.container(container.id).stop()
        .await
        .unwrap();
}