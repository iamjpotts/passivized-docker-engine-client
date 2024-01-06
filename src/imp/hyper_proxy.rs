use http_body_util::Full;
use hyper::{Request, Response};
use hyper::body::{Bytes, Incoming};
use hyper_tls::HttpsConnector;
use hyper_tls::native_tls::TlsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
#[cfg(unix)]
use hyperlocal::UnixConnector;

use crate::imp::hyper_shims::default_executor;

const POOL_MAX_IDLE_PER_HOST: usize = 10;

/// A thin proxy class that abstracts details and idiosyncrasies of the Hyper http client library.
///
/// Presents a unified interface for interacting with http servers, https servers, and unix sockets.
#[derive(Debug, Clone)]
pub(crate) struct HyperHttpClient {
    config: HyperHttpClientConfig
}

impl HyperHttpClient {

    pub(super) async fn apply(&self, request: Request<Full<Bytes>>) -> Result<Response<Incoming>, hyper_util::client::legacy::Error> {
        let future = match self.config {
            HyperHttpClientConfig::Http { ref client, ..} =>
                client.request(request),

            HyperHttpClientConfig::Https { ref client, ..} =>
                client.request(request),

            #[cfg(unix)]
            HyperHttpClientConfig::Unix { ref client, ..} =>
                client.request(request)

        };

        future.await
    }

    pub(crate) fn http() -> Self {
        Self {
            config: HyperHttpClientConfig::http()
        }
    }

    pub(crate) fn https(tls: TlsConnector) -> Self {
        Self {
            config: HyperHttpClientConfig::https(tls)
        }
    }

    #[cfg(unix)]
    pub(crate) fn unix() -> Self {
        Self {
            config: HyperHttpClientConfig::unix()
        }
    }

}

/// Internal configuration of the proxy class.
#[derive(Debug, Clone)]
enum HyperHttpClientConfig {
    Http {
        client: Client<HttpConnector, Full<Bytes>>,
    },
    Https {
        client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    },
    #[cfg(unix)]
    Unix {
        client: Client<UnixConnector, Full<Bytes>>,
    }
}

impl HyperHttpClientConfig {
    fn http() -> Self {
        Self::Http {
            client: Client::builder(default_executor())
                .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
                .build::<_, Full<Bytes>>(HttpConnector::new())
        }
    }

    fn https(tls: TlsConnector) -> Self {
        let mut inner = HttpConnector::new();
        inner.enforce_http(false);

        let connector = HttpsConnector::from((inner, tls.into()));

        Self::Https {
            client: Client::builder(default_executor())
                .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
                .build::<_, Full<Bytes>>(connector)
        }
    }

    #[cfg(unix)]
    fn unix() -> Self {
        Self::Unix {
            client: Client::builder(default_executor())
                .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
                .build::<_, Full<Bytes>>(UnixConnector {})
        }
    }

}

#[cfg(test)]
mod test_hyper_http_client {
    use http_body_util::Full;
    use hyper::body::Bytes;
    use hyper::{Request, StatusCode};

    use crate::imp::hyper_proxy::HyperHttpClient;

    #[tokio::test]
    async fn gets_from_http_server() {
        let mut server = mockito::Server::new_async().await;
        let path = "/some/file";

        server.mock("GET", path)
            .with_status(200)
            .create_async()
            .await;

        let client = HyperHttpClient::http();

        let request = Request::get(format!("{}{}", server.url(), path))
            .body(Full::new(Bytes::new()))
            .unwrap();

        let response = client
            .apply(request)
            .await
            .unwrap();

        assert_eq!(StatusCode::OK, response.status());
    }

}