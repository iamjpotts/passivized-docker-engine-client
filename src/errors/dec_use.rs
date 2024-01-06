use std::fmt::{Display, Formatter};
use std::string::FromUtf8Error;

use hyper::StatusCode;
use crate::imp::api::DockerEngineApiBuilderError;

use crate::errors::DecLibraryError;
use crate::model::StreamLineReadError;
use crate::imp::http_proxy::DockerEngineResponseNotUtf8;

/// An error during the use of a Docker Engine client.
#[derive(Debug)]
pub enum DecUseError {

    /// Received a 404 Not Found response for a list-based api.
    ///
    /// Possible causes are:
    ///   1. DockerEngineClient was misconfigured with an incorrect URL.
    ///   2. URL is correct but the Docker Engine it points to is incompatible with this library
    ApiNotFound {
        uri: String
    },

    /// Server returned 501 Not Implemented, most likely
    ///
    /// Possible causes are:
    ///   1. DockerEngineClient was misconfigured with an incorrect URL.
    ///   2. URL is correct but the Docker Engine it points to is incompatible with this library
    ApiNotImplemented {
        uri: String
    },

    /// A communication failure occurred while sending an HTTP request or receiving its response.
    HttpClientError(hyper::Error),

    HttpClientError2(hyper_util::client::legacy::Error),

    /// See docs for DecInternalError.
    Internal(DecLibraryError),

    /// An item managed by the Docker Engine, and required by the request,
    /// was not found (does not exist in the Docker Engine).
    ///
    /// For example, you requested to start a container, but that container
    /// does not exist.
    NotFound {
        /// Error message returned by the Docker Engine
        message: String
    },

    /// A problem while reading or parsing a container log or console output stream.
    StreamLineRead(StreamLineReadError),

    /// Docker Engine rejected the request. Any number of failure status
    /// codes can produce this outcome, including but not limited to:
    ///
    /// * 400 Bad Request
    /// * 409 Conflict
    /// * 500 Server Error
    ///
    /// However, this will not be used for 404 Not Found responses.
    Rejected {
        /// HTTP status returned by the Docker Engine
        status: StatusCode,

        /// Error message returned by the Docker Engine
        message: String
    },

    /// Received a response from the HTTP server with an unexpected or missing Content-Type.
    UnexpectedResponseContentType {
        expected: String,
        actual: Option<String>
    },

    /// A response was received, but could not be parsed.
    ///
    /// The two most likely causes:
    ///   1. The JSON was malformed (or not json at all)
    ///   2. The JSON was well formed, but the structure did not match the client's expectations
    ///
    /// A reverse-proxy related failure can cause #1 above. Malformed JSON from the actual Docker Engine is highly unlikely.
    ///
    /// This enum is a possible outcome for both HTTP success statuses and failure statuses.
    UnparseableJsonResponse {
        /// HTTP status returned by the Docker Engine or whatever HTTP server we were connected to
        status: StatusCode,

        /// Raw text from HTTP response body. May or may not be valid JSON
        text: String,

        /// Error that prevent conversion to the expected response data structure
        parse_error: serde_json::error::Error
    },

    /// A response was received, but could not be decoded as UTF-8.
    ///
    /// The most likely cause is a reverse-proxy related failure, and the
    /// reverse proxy sending back a non-UTF-8 plain text error message.
    /// Malformed UTF-8 from the actual Docker Engine is highly unlikely.
    ///
    /// This enum is a possible outcome for both HTTP success statuses and failure statuses.
    UnparseableUtf8Response {
        /// HTTP status returned by the Docker Engine or whatever HTTP server we were connected to
        status: StatusCode,

        /// HTTP Content-Type response header value returned by the Docker Engine or whatever HTTP server we were connected to
        content_type: Option<String>,

        /// Error that prevented converting the HTTP response body bytes into UTF-8 (the encoding of application/json).
        parse_error: FromUtf8Error
    },

}

impl DecUseError {

    pub fn error_message(&self) -> String {
        match self {
            Self::ApiNotFound { uri } =>
                format!("Api not found at {}", uri),

            Self::ApiNotImplemented { uri } =>
                format!("Api not implemented at {}", uri),

            Self::Internal(internal) =>
                internal.message(),

            Self::NotFound { message } =>
                message.clone(),

            Self::Rejected { status, message } =>
                format!("Request rejected with HTTP status: {}: {}", status, message),

            Self::StreamLineRead(error) =>
                error.error_message(),

            Self::UnparseableJsonResponse { status, text, parse_error} =>
                format!("Response with status {} had unparseable JSON: {}; response below:\n{}", status, parse_error, text),

            Self::UnparseableUtf8Response { status, content_type, parse_error } =>
                format!(
                    "Response with status {} and {} not parseable as UTF-8: {}",
                    status,
                    match content_type {
                        None => "no content type".into(),
                        Some(ct) => format!("content type {}", ct)
                    },
                    parse_error
                ),

            Self::HttpClientError(hyper_error) =>
                format!("Response error: {}", hyper_error),

            Self::HttpClientError2(error) =>
                format!("Response error: {}", error),

            Self::UnexpectedResponseContentType { expected, actual } =>
                format!(
                    "Expected response Content-Type of {} but {}",
                    expected,
                    match actual {
                        None =>
                            "header was missing".to_string(),

                        Some(a) =>
                            format!("received {}", a)
                    }
                )

        }
    }

    // A more explicit conversion than a From/Into trait pair. Prevents misconversion of error status responses.
    pub fn from_not_utf8(other: DockerEngineResponseNotUtf8) -> Self {
        Self::UnparseableUtf8Response {
            status: other.status,
            content_type: other.content_type,
            parse_error: other.error
        }
    }
}

impl Display for DecUseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error_message())
    }
}

impl From<DecLibraryError> for DecUseError {
    fn from(other: DecLibraryError) -> Self {
        Self::Internal(other)
    }
}

impl From<DockerEngineApiBuilderError> for DecUseError {
    fn from(other: DockerEngineApiBuilderError) -> Self {
        DecLibraryError::from(other).into()
    }
}

impl From<StreamLineReadError> for DecUseError {
    fn from(other: StreamLineReadError) -> Self {
        Self::StreamLineRead(other)
    }
}

impl From<url::ParseError> for DecUseError {
    fn from(other: url::ParseError) -> Self {
        DecLibraryError::from(other).into()
    }
}

#[cfg(test)]
mod test_error_message_and_display {
    use crate::errors::DecUseError;

    #[test]
    pub fn response_content_type_missing() {
        let error = DecUseError::UnexpectedResponseContentType {
            expected: "foo".into(),
            actual: None
        };

        let actual = format!("{}", error);

        assert_eq!("Expected response Content-Type of foo but header was missing".to_string(), actual);
    }

    #[test]
    pub fn response_content_type_wrong() {
        let error = DecUseError::UnexpectedResponseContentType {
            expected: "bar".into(),
            actual: Some("qux".into())
        };

        let actual = format!("{}", error);

        assert_eq!("Expected response Content-Type of bar but received qux".to_string(), actual);
    }
}
