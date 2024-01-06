use std::fmt::{Display, Formatter};

use crate::imp::api::DockerEngineApiBuilderError;

/// An error occurred during use of the Docker Engine client, and the error
/// is likely a bug of this library, as opposed to an external condition
/// such as a broken TCP connection or a misconfigured container.
#[derive(Debug)]
pub enum DecLibraryError {

    /// Failed to build a request object for the underlying HTTP client library.
    HttpRequestBuilderError(hyper::http::Error),

    /// Failed to encode registry authentication as json.
    RegistryAuthJsonEncodingError(serde_json::Error),

    /// Failed to convert a request into JSON.
    RequestSerializationError(serde_json::Error),

    /// Failed to serialize JSON while building a URL.
    UrlBuilderJsonError(serde_json::Error),

    /// Failed to build a URL.
    UrlBuilderParseError(url::ParseError),

}

impl DecLibraryError {

    pub fn message(&self) -> String {
        match self {
            Self::HttpRequestBuilderError(hyper_error) =>
                format!("Request error: {}", hyper_error),

            Self::RegistryAuthJsonEncodingError(error) =>
                format!("Failed to encode registry authentication as json: {}", error),

            Self::RequestSerializationError(serde_error) =>
                format!("Failed to serialize request body to json: {}", serde_error),

            Self::UrlBuilderJsonError(json_error) =>
                format!("URL builder json encoding failed: {}", json_error),

            Self::UrlBuilderParseError(parse_error) =>
                format!("URL builder failed: {}", parse_error),

        }
    }
}

impl Display for DecLibraryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl From<DockerEngineApiBuilderError> for DecLibraryError {
    fn from(other: DockerEngineApiBuilderError) -> Self {
        match other {
            DockerEngineApiBuilderError::Json(json_error) =>
                DecLibraryError::UrlBuilderJsonError(json_error),

            DockerEngineApiBuilderError::Url(parse_error) =>
                DecLibraryError::UrlBuilderParseError(parse_error)
        }
    }
}

impl From<url::ParseError> for DecLibraryError {
    fn from(other: url::ParseError) -> Self {
        DecLibraryError::UrlBuilderParseError(other)
    }
}

#[cfg(test)]
mod test_error_message_and_display {
    use hyper::StatusCode;
    use super::DecLibraryError;

    fn simulated_json_failure() -> (String, serde_json::Error) {
        let result: Result<i32, serde_json::Error> = serde_json::from_str("");
        let error = result.unwrap_err();
        let message = format!("{}", error);

        (message, error)
    }

    #[test]
    fn request_builder_error() {
        let inner = StatusCode::from_u16(12345)
            .unwrap_err();

        let middle = hyper::http::Error::from(inner);
        let outer = DecLibraryError::HttpRequestBuilderError(middle);

        // This is a bit nonsensical but invalid status code was an easy way to create an error value for a test.
        assert_eq!("Request error: invalid status code", format!("{}", outer));
    }

    #[test]
    fn url_builder_json_encoding_failed() {
        let (message, json_error) = simulated_json_failure();
        let internal_error = DecLibraryError::UrlBuilderJsonError(json_error);

        let expected = format!("URL builder json encoding failed: {}", message);
        let actual = format!("{}", internal_error);

        assert_eq!(expected, actual);
    }
}