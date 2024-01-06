#[path = "test_utils/lib.rs"]
mod test_utils;

use std::str::FromStr;

use http::Uri;
use hyper::StatusCode;
use mockito::{Server, ServerGuard};
use tar::{Header, Builder};
use test_utils::images::web;
use test_utils::{content_type, random_name};
use passivized_docker_engine_client::client::DOCKER_ENGINE_VERSION;
use passivized_docker_engine_client::DockerEngineClient;
use passivized_docker_engine_client::errors::DecUseError;
use passivized_docker_engine_client::model::Tar;
use passivized_docker_engine_client::requests::BuildImageRequest;

#[cfg(not(windows))]
use passivized_docker_engine_client::model::TsStreamLine;

const MISSING_IMAGE: &str = "does_not_exist";
const MISSING_TAG: &str = "does_not_exist";

fn mockito_client(server: &ServerGuard) -> DockerEngineClient {
    DockerEngineClient::with_server(server.url())
        .unwrap()
}

#[tokio::test]
async fn test_bad_image_list_responses() {
    let mut server = Server::new_async().await;

    let dec = mockito_client(&server);

    let path = format!("/{}/images/json", DOCKER_ENGINE_VERSION);

    {
        let error = dec.images().list().await.unwrap_err();

        if let DecUseError::ApiNotImplemented { uri } = error {
            assert_eq!(path, Uri::from_str(&uri).unwrap().path());
        }
        else {
            panic!("Did not expect {}", error)
        }
    }

    server.reset();

    server.mock("GET", path.as_str())
        .with_status(404)
        .create_async()
        .await;

    {
        let error = dec.images().list().await.unwrap_err();

        match error {
            DecUseError::ApiNotFound { uri} => {
                assert_eq!(path, Uri::from_str(&uri).unwrap().path());
            }
            _ => {
                panic!("Did not expect {}", error)
            }
        }
    }

    server.reset();

    server.mock("GET", path.as_str())
        .with_status(200)
        .with_header("Content-Type", content_type::JSON)
        .with_body("not json")
        .create_async()
        .await;

    {
        let error = dec.images().list().await.unwrap_err();

        match error {
            DecUseError::UnparseableJsonResponse { status, text, parse_error } => {
                assert_eq!(200, status);
                assert_eq!("not json", &text);
                assert_eq!("expected ident at line 1 column 2", &parse_error.to_string());
            }
            _ => {
                panic!("Did not expect {}", error)
            }
        }
    }
}

#[cfg(not(windows))]  // Due to use of WaitCondition::NotRunning
#[tokio::test]
async fn test_build_and_run() {
    use time::OffsetDateTime;
    use passivized_docker_engine_client::requests::WaitCondition;
    use passivized_docker_engine_client::requests::CreateContainerRequest;

    const FN: &str = "test_build_and_run";

    let started_at = OffsetDateTime::now_utc();

    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present("busybox", "latest")
        .await
        .unwrap();

    let mut builder = Builder::new(Vec::new());

    {
        let docker_file_text = "FROM busybox\n\nCMD [\"echo\", \"musk\", \"bought\", \"twitter\"]";
        let docker_file = docker_file_text.as_bytes();

        let mut header = Header::new_gnu();
        header.set_path("Dockerfile")
            .unwrap();
        header.set_size(docker_file.len() as u64);
        header.set_cksum();

        builder.append(&header, docker_file)
            .unwrap();
    }

    let archive = Tar(builder.into_inner()
        .unwrap());

    let tag = "test_images:latest";

    let request = BuildImageRequest::default()
        .tag(tag);

    dec.images().build(request, archive)
        .await
        .unwrap();

    let container_name = random_name(FN);

    let create_request = CreateContainerRequest::default()
        .image(tag)
        .name(container_name);

    let container = dec.containers().create(create_request)
        .await
        .unwrap();

    dec.container(&container.id).start()
        .await
        .unwrap();

    dec.container(&container.id).wait(WaitCondition::NotRunning)
        .await
        .unwrap();

    let logs = dec.container(&container.id).logs()
        .await
        .unwrap();

    for line in &logs {
        println!("{}", line.text);
    }

    let ts_logs: Vec<TsStreamLine> = dec.container(&container.id).logs_timestamped()
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    dec.container(&container.id).remove()
        .await
        .unwrap();

    dec.images().untag(tag)
        .await
        .unwrap();

    let ended_at = OffsetDateTime::now_utc();

    assert!(logs.iter().any(|line| line.text == "musk bought twitter\n"));

    let found = ts_logs
        .iter()
        .find(|line| line.text == "musk bought twitter\n")
        .unwrap();

    assert!(started_at < found.timestamp);
    assert!(found.timestamp < ended_at);
}

#[tokio::test]
async fn test_build_fails_invalid_from() {
    let dec = DockerEngineClient::new()
        .unwrap();

    let mut builder = Builder::new(Vec::new());

    {
        let docker_file_text = "FROM doesnotexist.locallan/doesnotexist\n\nRUN echo Hi\n";
        let docker_file = docker_file_text.as_bytes();

        let mut header = Header::new_gnu();
        header.set_path("Dockerfile")
            .unwrap();
        header.set_size(docker_file.len() as u64);
        header.set_cksum();

        builder.append(&header, docker_file)
            .unwrap();
    }

    let archive = Tar(builder.into_inner()
        .unwrap());

    let request = BuildImageRequest::default()
        .tag("test_images_fail_from:latest");

    let actual = dec.images().build(request, archive)
        .await
        .unwrap();

    let error = actual
        .iter()
        .find(|item| item.error.is_some())
        .unwrap();

    let allowed = [
        // Linux and Windows variant
        ("dial tcp: lookup doesnotexist.locallan", "no such host"),
        // Mac variant on GitHub runner
        ("Failed to lookup host", "doesnotexist.locallan"),
        // Mac variant on laptop
        ("resolving host doesnotexist.locallan", "no such host")
    ];

    let error_error = error.error.as_ref().unwrap();
    let error_detail = &error.error_detail.as_ref().unwrap().message;

    let (a, b) = allowed
        .into_iter()
        .find(|(a, b)| error_error.contains(a) && error_error.contains(b))
        .expect(&format!("Did not find any allowed matches for error.error: {error_error}"));

    assert_contains_both("error_detail", a, b, &error_detail);
}

fn assert_contains_both(what: &str, expected_first: &str, expected_second: &str, actual: &str) {
    assert!(
        actual.contains(expected_first) && actual.contains(expected_second),
        "Expected in {}: {} and {}\n  Actual: {:?}",
        what,
        expected_first,
        expected_second,
        actual
    );
}

#[tokio::test]
async fn test_build_fails_invalid_syntax() {
    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present("busybox", "latest")
        .await
        .unwrap();

    let mut builder = Builder::new(Vec::new());

    {
        // Invalid syntax for a Dockerfile
        let docker_file_text = "&%23%$!";
        let docker_file = docker_file_text.as_bytes();

        let mut header = Header::new_gnu();
        header.set_path("Dockerfile")
            .unwrap();
        header.set_size(docker_file.len() as u64);
        header.set_cksum();

        builder.append(&header, docker_file)
            .unwrap();
    }

    let archive = Tar(builder.into_inner()
        .unwrap());

    let request = BuildImageRequest {
        tags: vec!["test_images_fail:latest".into()],
        ..BuildImageRequest::default()
    };

    let actual = dec.images().build(request, archive)
        .await
        .unwrap_err();

    if let DecUseError::Rejected { status, message } = actual {
        assert_eq!(StatusCode::BAD_REQUEST, status);
        assert!(message.contains("dockerfile parse error"), "Message contains 'dockerfile parse error': {}", message);
    }
    else {
        panic!("Unexpected error: {}", actual);
    }
}

#[tokio::test]
async fn test_pull_and_list_images() {
    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present(web::IMAGE, web::TAG)
        .await
        .unwrap();

    let images = dec.images().list()
        .await
        .unwrap();

    let found = images
        .iter()
        .find(|i| i.repo_tags.contains(&format!("{}:{}", web::IMAGE, web::TAG)))
        .unwrap();

    assert_eq!(web::HASH, found.id);
    assert!(found.size > web::MIN_SIZE);
}

#[tokio::test]
async fn test_pull_not_found() {
    let dec = DockerEngineClient::new()
        .unwrap();

    let error = dec.images().pull(MISSING_IMAGE, MISSING_TAG)
        .await
        .unwrap_err();

    if let DecUseError::NotFound { message} = error {
        assert!(message.contains("does not exist"));
        assert!(message.contains(MISSING_IMAGE));
    }
    else {
        panic!("Did not expect {}", error)
    }
}

#[tokio::test]
async fn test_tag_and_untag() {
    let dec = DockerEngineClient::new()
        .unwrap();

    dec.images().pull_if_not_present(web::IMAGE, web::TAG)
        .await
        .unwrap();

    let image_name = format!("{}:{}", web::IMAGE, web::TAG);

    let new_repo = random_name(web::IMAGE);
    let new_tag = random_name(web::TEST_TAG_PREFIX);

    dec.images().tag(image_name, &new_repo, &new_tag)
        .await
        .unwrap();

    let new_image = format!("{}:{}", new_repo, new_tag);

    let found = dec.images().list()
        .await
        .unwrap()
        .iter()
        .filter(|item| item.repo_tags.contains(&new_image))
        .next()
        .is_some();

    assert!(found, "{}", new_image);

    // Verify untag defaults to foo:latest, and will return a not found error when appropriate

    let untag_error = dec.images().untag(&new_repo)
        .await
        .unwrap_err();

    if let DecUseError::NotFound { message } = untag_error {
        let expected = format!("No such image: {}:latest", new_repo);
        assert_eq!(expected, message);
    }
    else {
        panic!("Unexpected untag error: {:?}", untag_error);
    }

    // Verify successful untag

    dec.images().untag(&new_image)
        .await
        .unwrap();

    let found_after_untag = dec.images().list()
        .await
        .unwrap()
        .iter()
        .filter(|item| item.repo_tags.contains(&new_image))
        .next()
        .is_some();

    assert!(!found_after_untag, "{}", new_image);
}

#[tokio::test]
async fn test_tag_source_not_found() {
    let dec = DockerEngineClient::new()
        .unwrap();

    let old_image_name = random_name("does_not_exist");
    let new_repo = random_name("will_not_be_created");

    let tag_error = dec.images().tag(&old_image_name, &new_repo, "latest")
        .await
        .unwrap_err();

    if let DecUseError::NotFound { message } = tag_error {
        let expected = format!("No such image: {}:latest", old_image_name);
        assert_eq!(expected, message);
    }
    else {
        panic!("Unexpected tag error: {:?}", tag_error);
    }
}
