use super::*;

impl QUICListener {
    pub(super) fn handle_metrics_request(
        req: Request<Incoming>,
        metrics_path: &str,
        metrics: Arc<Metrics>,
    ) -> Response<Full<Bytes>> {
        if req.uri().path() != metrics_path {
            return Self::metrics_not_found_response();
        }

        Self::metrics_ok_response(metrics.render_prometheus())
    }

    fn metrics_not_found_response() -> Response<Full<Bytes>> {
        match Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"not found\n")))
        {
            Ok(resp) => resp,
            Err(_) => Response::new(Full::new(Bytes::from_static(b"not found\n"))),
        }
    }

    fn metrics_ok_response(body: String) -> Response<Full<Bytes>> {
        match Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/plain; version=0.0.4")
            .body(Full::new(Bytes::from(body)))
        {
            Ok(resp) => resp,
            Err(_) => Response::new(Full::new(Bytes::from_static(b"failed to render metrics\n"))),
        }
    }
}

#[cfg(test)]
mod tests {
    // Domain anchor: metrics endpoint path and response handling contract tests
    // live here so they stay local to the HTTP handling surface rather than
    // drifting into listener/control-plane integration coverage.
}
