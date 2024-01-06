use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::Response;
use hyper_util::rt::TokioExecutor;

pub(super) fn default_executor() -> TokioExecutor {
    TokioExecutor::new()
}

pub(super) async fn incoming_bytes(mut response: Response<Incoming>) -> Result<Vec<u8>, hyper::Error> {
    let mut response_body: Vec<u8> = Vec::new();

    while let Some(frame_result) = response.frame().await {
        let frame = frame_result?;

        if let Some(segment) = frame.data_ref() {
            response_body.extend_from_slice(segment.iter().as_slice());
        }
    }

    Ok(response_body)
}
