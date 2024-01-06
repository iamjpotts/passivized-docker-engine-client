use std::collections::HashMap;
use std::string::FromUtf8Error;
use http_body_util::Full;
use hyper::http::header::CONTENT_TYPE;

use hyper::{Request, StatusCode};
use hyper::body::Bytes;
use log::debug;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Deserializer;

use crate::errors::{DecLibraryError, DecUseError};
use crate::imp::content_type;
use crate::imp::hyper_proxy::HyperHttpClient;
use crate::imp::hyper_shims::incoming_bytes;
use crate::imp::other::{base64_encode, converge};
use crate::model::{RegistryAuth, RegistryConfig};
use crate::responses::ErrorResponse;

/// A proxy class that provides a basic REST-based DSL for interacting
/// with a Docker Engine HTTP API endpoint. Most payloads are JSON.
#[derive(Clone, Debug)]
pub(crate) struct DockerEngineHttpClient {
    client: HyperHttpClient
}

impl DockerEngineHttpClient {

    pub fn new(client: HyperHttpClient) -> Self {
        Self {
            client
        }
    }

    fn build_delete(uri: &str) -> Result<Request<Full<Bytes>>, DecLibraryError> {
        Request::delete(uri)
            .body(Full::new(Bytes::new()))
            .map_err(DecLibraryError::HttpRequestBuilderError)
    }

    fn build_get(uri: &str) -> Result<Request<Full<Bytes>>, DecLibraryError> {
        Request::get(uri.to_string())
            .body(Full::new(Bytes::new()))
            .map_err(DecLibraryError::HttpRequestBuilderError)
    }

    #[cfg(not(windows))]
    fn build_put(uri: &str, content_type: &str, content: Vec<u8>) -> Result<Request<Full<Bytes>>, DecLibraryError> {
        Request::put(uri)
            .header(CONTENT_TYPE, content_type)
            .body(Full::new(content.into()))
            .map_err(DecLibraryError::HttpRequestBuilderError)
    }

    fn build_post_json<B: Serialize>(uri: &str, body: &B) -> Result<Request<Full<Bytes>>, DecLibraryError> {
        let json = serde_json::to_string(body)
            .map_err(DecLibraryError::RequestSerializationError)?;

        let body = Bytes::from(json);

        Request::post(uri.to_string())
            .header(CONTENT_TYPE, content_type::JSON)
            .body(Full::new(body))
            .map_err(DecLibraryError::HttpRequestBuilderError)
    }

    fn build_post_with_auth(uri: &str, registry_auth: &Option<RegistryAuth>) -> Result<Request<Full<Bytes>>, DecLibraryError> {
        let mut builder = Request::post(uri.to_string());

        if let Some(value) = Self::x_registry_auth(registry_auth)? {
            builder = builder.header("X-Registry-Auth", value);
        }

        builder
            .body(Full::new(Bytes::new()))
            .map_err(DecLibraryError::HttpRequestBuilderError)
    }

    fn build_post_with_auth_config(
        uri: &str,
        registry_config: &HashMap<String, RegistryConfig>,
        content_type: &str,
        body: Vec<u8>) -> Result<Request<Full<Bytes>>, DecLibraryError>
    {
        let mut builder = Request::post(uri.to_string())
            .header(CONTENT_TYPE, content_type);

        if let Some(value) = Self::x_registry_config(registry_config)? {
            builder = builder.header("X-Registry-Config", value);
        }

        builder
            .body(Full::new(body.into()))
            .map_err(DecLibraryError::HttpRequestBuilderError)
    }

    fn build_request<U, F>(&self, uri: U, request_from_uri: F) -> Result<DockerEngineHttpRequest, DecLibraryError>
    where
        U: ToString,
        F: FnOnce(&str) -> Result<Request<Full<Bytes>>, DecLibraryError>
    {
        let u = uri.to_string();

        Ok(DockerEngineHttpRequest {
            client: self.client.clone(),
            request: request_from_uri(&u)?,
            uri: u
        })
    }

    pub fn delete<U: ToString>(&self, uri: U) -> Result<DockerEngineHttpRequest, DecLibraryError> {
        self.build_request(uri, Self::build_delete)
    }

    pub fn get<U: ToString>(&self, uri: U) -> Result<DockerEngineHttpRequest, DecLibraryError> {
        self.build_request(uri, Self::build_get)
    }

    pub fn post<U: ToString>(&self, uri: U) -> Result<DockerEngineHttpRequest, DecLibraryError> {
        self.build_request(uri, |u| Self::build_post_with_auth(u, &None))
    }

    pub fn post_json<U: ToString, B: Serialize>(&self, uri: U, body: &B) -> Result<DockerEngineHttpRequest, DecLibraryError> {
        self.build_request(uri, |u| Self::build_post_json(u, body))
    }

    pub fn post_with_auth<U: ToString>(&self, uri: U, registry_auth: &Option<RegistryAuth>) -> Result<DockerEngineHttpRequest, DecLibraryError> {
        self.build_request(uri, |u| Self::build_post_with_auth(u, registry_auth))
    }

    pub fn post_with_auth_config<U: ToString>(
        &self,
        uri: U,
        registry_auth: &Option<RegistryAuth>,
        content_type: &str,
        body: Vec<u8>
    ) -> Result<DockerEngineHttpRequest, DecLibraryError>
    {
        let auth_config = registry_auth
            .as_ref()
            .map(|ra| ra.as_config())
            .unwrap_or_default();

        self.build_request(uri, |u| Self::build_post_with_auth_config(u, &auth_config, content_type, body))
    }

    #[cfg(not(windows))]
    pub fn put<U: ToString>(&self, uri: U, content_type: &str, content: Vec<u8>) -> Result<DockerEngineHttpRequest, DecLibraryError> {
        self.build_request(uri, |u| Self::build_put(u, content_type, content))
    }

    fn x_registry_auth(registry_auth: &Option<RegistryAuth>) -> Result<Option<String>, DecLibraryError> {
        match registry_auth {
            None => Ok(None),
            Some(auth) => {
                let json = serde_json::to_string(&auth)
                    .map_err(DecLibraryError::RegistryAuthJsonEncodingError)?;

                Ok(Some(base64_encode(json)))
            }
        }
    }

    fn x_registry_config(registry_config: &HashMap<String, RegistryConfig>) -> Result<Option<String>, DecLibraryError> {
        if registry_config.is_empty() {
            Ok(None)
        }
        else {
            let json = serde_json::to_string(registry_config)
                .map_err(DecLibraryError::RegistryAuthJsonEncodingError)?;

            Ok(Some(base64_encode(json)))
        }
    }
}

#[derive(Debug)]
pub(crate) struct DockerEngineHttpRequest {
    client: HyperHttpClient,
    request: Request<Full<Bytes>>,
    uri: String
}

impl DockerEngineHttpRequest {

    pub async fn execute(self) -> Result<DockerEngineHttpResponse, DecUseError> {
        let response = self.client
            .apply(self.request)
            .await
            .map_err(DecUseError::HttpClientError2)?;

        Ok(
            DockerEngineHttpResponse {
                request_uri: self.uri,
                status: response.status(),
                content_type: match response.headers().get("Content-Type") {
                    None => None,
                    Some(hv) => match hv.to_str() {
                        Ok(text) => Some(text.to_string()),
                        Err(_) => None
                    }
                },
                body: incoming_bytes(response)
                    .await
                    .map_err(DecUseError::HttpClientError)?
                    .into()
            }
        )
    }

}

#[derive(Clone, Debug)]
pub(crate) struct DockerEngineHttpResponse {
    pub(crate) request_uri: String,
    pub(crate) status: StatusCode,
    pub(crate) content_type: Option<String>,
    pub(crate) body: Bytes,
}

impl DockerEngineHttpResponse {

    pub(crate) fn assert_item_status(self: DockerEngineHttpResponse, expected: StatusCode) -> Result<DockerEngineHttpResponse, DecUseError> {
        self.assert_item_status_in(&[expected])
    }

    pub(crate) fn assert_item_status_in(self: DockerEngineHttpResponse, expected: &[StatusCode]) -> Result<DockerEngineHttpResponse, DecUseError> {
        if expected.contains(&self.status) {
            Ok(self)
        }
        else {
            Err(self.parse_other_item_response())
        }
    }

    pub(crate) fn assert_list_status(self: DockerEngineHttpResponse, expected: StatusCode) -> Result<DockerEngineHttpResponse, DecUseError> {
        if self.status == expected {
            Ok(self)
        }
        else {
            Err(self.parse_other_list_response())
        }
    }

    pub(crate) fn assert_unit_status(self: DockerEngineHttpResponse, expected: StatusCode) -> Result<(), DecUseError> {
        self.assert_unit_status_in(&[expected])
    }

    pub(crate) fn assert_unit_status_in(self: DockerEngineHttpResponse, expected: &[StatusCode]) -> Result<(), DecUseError> {
        self.assert_item_status_in(expected)?;

        Ok(())
    }

    fn assume_utf8(&self) -> Result<String, DockerEngineResponseNotUtf8> {
        // With the exception of exec output, and file copies, all responses are application/json,
        // which has a default encoding of UTF-8 according to RFC4627.
        // https://www.ietf.org/rfc/rfc4627.txt
        String::from_utf8(self.body.to_vec())
            .map_err(|e|
                DockerEngineResponseNotUtf8 {
                    status: self.status,
                    content_type: self.content_type.clone(),
                    error: e
                }
            )
    }

    pub(crate) fn assert_content_type(self, expected: &str) -> Result<Self, DecUseError> {
        if self.content_type == Some(expected.to_string()) {
            Ok(self)
        }
        else {
            Err(DecUseError::UnexpectedResponseContentType {
                expected: expected.to_string(),
                actual: self.content_type
            })
        }
    }

    pub fn assume_content_type(self, expected: &str) -> Result<Self, DecUseError> {
        if self.content_type.is_none() {
            Ok(self)
        }
        else {
            self.assert_content_type(expected)
        }
    }

    fn assert_json_text(self) -> Result<String, DecUseError> {
        self
            .assert_content_type(content_type::JSON)?
            .assume_utf8()
            .map_err(DecUseError::from_not_utf8)
    }

    pub(crate) fn parse_stream<A: DeserializeOwned>(self) -> Result<Vec<A>, DecUseError> {
        let status = self.status;
        let request_uri = self.request_uri.clone();
        let body = self.assert_json_text()?;
        let parser = Deserializer::from_str(&body).into_iter::<A>();

        let mut result: Vec<A> = Vec::new();

        for parse_result in parser {
            let parsed = parse_result
                .map_err(|e| Self::parsing_error(request_uri.clone(), status, body.clone(), e))?;

            result.push(parsed)
        }

        Ok(result)
    }

    pub(crate) fn parse<A: DeserializeOwned>(self) -> Result<A, DecUseError> {
        let status = self.status;
        let request_uri = self.request_uri.clone();
        let body = self.assert_json_text()?;
        let parse_result = serde_json::from_str(&body);

        parse_result
            .map_err(|e| Self::parsing_error(request_uri, status, body, e))
    }

    fn parsing_error(request_uri: String, status: StatusCode, body: String, e: serde_json::Error) -> DecUseError {
        debug!(
            "Failed to parse {} received from {}: {}\n{}",
            content_type::JSON,
            request_uri,
            e,
            body
        );

        DecUseError::UnparseableJsonResponse {
            status,
            text: body,
            parse_error: e
        }
    }

    /// Parse a response where either:
    /// a) the URL asserts that something exists, such as a container or image, at its path, or
    /// b) the URL is for creating a new item (possibly a sub item of a parent item)
    ///
    /// If a 404 Not Found is returned, its most likely because the item (e.g. container or image)
    /// is not present. For a creation URL, its possible the API does not exist at that path either,
    /// but for now we do not distinguish that. It is not known if the Docker Engine will return
    /// a 404 Not Found error if we have the right URL, but a dependency of what we are creating
    /// (such as an existing network required by a new container) does not exist.
    fn parse_other_item_response(self) -> DecUseError {
        let parse = move || -> Result<DecUseError, DecUseError> {
            Ok(match self.status {
                StatusCode::NOT_FOUND => {
                    let parsed: ErrorResponse = self.parse()?;

                    DecUseError::NotFound {
                        message: parsed.message
                    }
                },

                StatusCode::NOT_IMPLEMENTED =>
                    DecUseError::ApiNotImplemented { uri: self.request_uri },

                _ => self.unexpected_status()
            })
        };

        converge(parse())
    }

    /// Parse a response where the URL only asserts that a list-based API exists its path.
    ///
    /// If a 404 Not Found is returned, its most likely because a Docker Engine does not exist
    /// at the base URL the client was provided.
    pub(crate) fn parse_other_list_response(self) -> DecUseError {
        let parse = move || -> Result<DecUseError, DecUseError> {
            Ok(match self.status {
                StatusCode::NOT_FOUND =>
                    DecUseError::ApiNotFound { uri: self.request_uri },

                StatusCode::NOT_IMPLEMENTED =>
                    DecUseError::ApiNotImplemented { uri: self.request_uri },

                _ => self.unexpected_status()
            })
        };

        converge(parse())
    }

    /// Syntax sugar for calling a custom parser after asserting a status code.
    ///
    /// Prevents creating an intermediate variable at the call site of the same
    /// type as the original.
    pub(crate) fn parse_with<A, P>(self, parser: P) -> Result<A, DecUseError>
    where
        P: FnOnce(Self) -> Result<A, DecUseError> {

        parser(self)
    }

    fn unexpected_status(self) -> DecUseError {
        let status = self.status;
        let parse_result: Result<ErrorResponse, DecUseError> = self.parse();

        match parse_result {
            Err(e) => e,
            Ok(parsed) =>
                DecUseError::Rejected {
                    status,
                    message: parsed.message
                }
        }
    }

}

#[derive(Clone, Debug)]
pub struct DockerEngineResponseNotUtf8 {
    pub status: StatusCode,
    pub content_type: Option<String>,
    pub error: FromUtf8Error
}

#[cfg(test)]
mod test_der {
    use hyper::body::Bytes;
    use hyper::StatusCode;

    use super::DockerEngineHttpResponse;

    fn arbitrary() -> DockerEngineHttpResponse {
        DockerEngineHttpResponse {
            request_uri: "foo".into(),
            status: StatusCode::from_u16(123).unwrap(),
            content_type: Some("arbitrary".into()),
            body: Bytes::from(vec![123])
        }
    }

    mod assert_item_status {
        use hyper::body::Bytes;
        use hyper::StatusCode;
        use crate::errors::DecUseError;
        use crate::imp::content_type::JSON;
        use crate::imp::http_proxy::DockerEngineHttpResponse;

        #[test]
        fn fails_when_item_not_found() {
            let response = DockerEngineHttpResponse {
                content_type: Some(JSON.into()),
                status: StatusCode::NOT_FOUND,
                body: Bytes::from(&b"{ \"message\": \"missing\" }"[..]),
                ..super::arbitrary()
            };

            let actual = response.assert_item_status(StatusCode::OK)
                .unwrap_err();

            if let DecUseError::NotFound { message } = actual {
                assert_eq!("missing", message);
            }
            else {
                panic!("Unexpected error: {}", actual);
            }
        }

        #[test]
        fn fails_and_parses_docker_json_error_when_different() {
            let response = DockerEngineHttpResponse {
                content_type: Some(JSON.into()),
                status: StatusCode::CREATED,
                body: Bytes::from(&b"{ \"message\": \"boom\" }"[..]),
                ..super::arbitrary()
            };

            let actual = response.assert_item_status(StatusCode::ACCEPTED)
                .unwrap_err();

            if let DecUseError::Rejected { status, message } = actual {
                assert_eq!(StatusCode::CREATED, status);
                assert_eq!("boom", message);
            }
            else {
                panic!("Unexpected error: {}", actual);
            }
        }

        #[test]
        fn passes_when_equal() {
            let response = DockerEngineHttpResponse {
                status: StatusCode::NOT_MODIFIED,
                ..super::arbitrary()
            };

            response.assert_item_status(StatusCode::NOT_MODIFIED)
                .unwrap();
        }
    }

    mod assert_item_status_in {
        use hyper::body::Bytes;
        use hyper::StatusCode;
        use crate::errors::DecUseError;
        use crate::imp::content_type::JSON;
        use crate::imp::http_proxy::DockerEngineHttpResponse;

        #[test]
        fn fails_and_parses_docker_json_error_when_no_match() {
            let response = DockerEngineHttpResponse {
                content_type: Some(JSON.into()),
                status: StatusCode::CREATED,
                body: Bytes::from(&b"{ \"message\": \"boom\" }"[..]),
                ..super::arbitrary()
            };

            let actual = response.assert_item_status_in(&[StatusCode::OK, StatusCode::ACCEPTED])
                .unwrap_err();

            if let DecUseError::Rejected { status, message } = actual {
                assert_eq!(StatusCode::CREATED, status);
                assert_eq!("boom", message);
            }
            else {
                panic!("Unexpected error: {}", actual);
            }
        }

        #[test]
        fn passes_when_matched() {
            let response = DockerEngineHttpResponse {
                status: StatusCode::NOT_MODIFIED,
                ..super::arbitrary()
            };

            response.assert_item_status_in(&[StatusCode::CREATED, StatusCode::NOT_MODIFIED])
                .unwrap();
        }
    }

    mod assert_list_status {
        use hyper::StatusCode;
        use crate::errors::DecUseError;
        use crate::imp::http_proxy::DockerEngineHttpResponse;

        #[test]
        fn fails_when_api_not_found() {
            let response = DockerEngineHttpResponse {
                request_uri: "some-uri".into(),
                content_type: None,
                status: StatusCode::NOT_FOUND,
                body: Default::default(),
                ..super::arbitrary()
            };

            let actual = response.assert_list_status(StatusCode::OK)
                .unwrap_err();

            if let DecUseError::ApiNotFound { uri } = actual {
                assert_eq!("some-uri", uri);
            }
            else {
                panic!("Unexpected error: {}", actual);
            }
        }

    }

    mod assume_utf8 {
        use hyper::body::Bytes;

        use super::super::DockerEngineHttpResponse;

        #[test]
        fn valid_utf8() {
            let response = DockerEngineHttpResponse {
                body: Bytes::from(vec![65]),
                ..super::arbitrary()
            };

            let actual = response.assume_utf8()
                .unwrap();

            assert_eq!("A", actual);
        }

        #[test]
        fn invalid_utf8() {
            let response = DockerEngineHttpResponse {
                body: Bytes::from(vec![0xc3, 0x28]),
                ..super::arbitrary()
            };

            let actual = response.assume_utf8()
                .unwrap_err();

            assert_eq!(123, actual.status.as_u16());
            assert_eq!(Some("arbitrary".to_string()), actual.content_type);
            assert_eq!("invalid utf-8 sequence of 1 bytes from index 0".to_string(), format!("{}", actual.error));
        }
    }

    mod assert_content_type {
        use crate::errors::DecUseError;
        use super::super::DockerEngineHttpResponse;

        #[test]
        fn err_when_none() {
            let response = DockerEngineHttpResponse {
                content_type: None,
                ..super::arbitrary()
            };

            let result = response.assert_content_type("foo")
                .unwrap_err();

            if let DecUseError::UnexpectedResponseContentType { expected, actual } = result {
                assert_eq!("foo", expected);
                assert_eq!(None, actual);
            }
            else {
                panic!("Unexpected result: {}", result)
            }
        }

        #[test]
        fn ok_when_match() {
            let response = DockerEngineHttpResponse {
                content_type: Some("bar".to_string()),
                ..super::arbitrary()
            };

            response.assert_content_type("bar")
                .unwrap();
        }

        #[test]
        fn err_when_different() {
            let response = DockerEngineHttpResponse {
                content_type: Some("qux".to_string()),
                ..super::arbitrary()
            };

            let result= response.assert_content_type("baz")
                .unwrap_err();

            if let DecUseError::UnexpectedResponseContentType { expected, actual } = result {
                assert_eq!("baz", expected);
                assert_eq!(Some("qux".to_string()), actual);
            }
            else {
                panic!("Unexpected result: {}", result)
            }
        }
    }

    mod assume_content_type {
        use crate::errors::DecUseError;
        use super::super::DockerEngineHttpResponse;

        #[test]
        fn ok_when_none() {
            let response = DockerEngineHttpResponse {
                content_type: None,
                ..super::arbitrary()
            };

            response.assume_content_type("foo")
                .unwrap();
        }

        #[test]
        fn ok_when_match() {
            let response = DockerEngineHttpResponse {
                content_type: Some("bar".to_string()),
                ..super::arbitrary()
            };

            response.assume_content_type("bar")
                .unwrap();
        }

        #[test]
        fn err_when_different() {
            let response = DockerEngineHttpResponse {
                content_type: Some("qux".to_string()),
                ..super::arbitrary()
            };

            let result= response.assume_content_type("baz")
                .unwrap_err();

            if let DecUseError::UnexpectedResponseContentType { expected, actual } = result {
                assert_eq!("baz", expected);
                assert_eq!(Some("qux".to_string()), actual);
            }
            else {
                panic!("Unexpected result: {}", result)
            }
        }
    }

    mod parse_other_item_response {
        use hyper::StatusCode;
        use crate::errors::DecUseError;
        use crate::imp::http_proxy::DockerEngineHttpResponse;

        #[test]
        fn maps_status_to_error_when_api_not_implemented() {
            let response = DockerEngineHttpResponse {
                request_uri: "bar".into(),
                status: StatusCode::NOT_IMPLEMENTED,
                content_type: Some("foo".into()),
                body: Default::default()
            };

            let actual = response.parse_other_item_response();

            if let DecUseError::ApiNotImplemented { uri} = actual {
                assert_eq!("bar", uri);
            }
            else {
                panic!("Unexpected result: {}", actual);
            }
        }
    }

    mod parse_other_list_response {
        use hyper::StatusCode;
        use crate::errors::DecUseError;
        use super::super::DockerEngineHttpResponse;

        #[test]
        fn maps_status_to_error_when_api_not_found() {
            let response = DockerEngineHttpResponse {
                request_uri: "bar".into(),
                status: StatusCode::NOT_FOUND,
                content_type: Some("foo".into()),
                body: Default::default()
            };

            let actual = response.parse_other_list_response();

            if let DecUseError::ApiNotFound { uri} = actual {
                assert_eq!("bar", uri);
            }
            else {
                panic!("Unexpected result: {}", actual);
            }
        }

        #[test]
        fn maps_status_to_error_when_api_not_implemented() {
            let response = DockerEngineHttpResponse {
                request_uri: "bar".into(),
                status: StatusCode::NOT_IMPLEMENTED,
                content_type: Some("foo".into()),
                body: Default::default()
            };

            let actual = response.parse_other_list_response();

            if let DecUseError::ApiNotImplemented { uri} = actual {
                assert_eq!("bar", uri);
            }
            else {
                panic!("Unexpected result: {}", actual);
            }
        }
    }

}