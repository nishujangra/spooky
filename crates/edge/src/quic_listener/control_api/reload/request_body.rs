use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Body;
use serde::{Deserialize, de::DeserializeOwned};

use super::*;

pub(super) const MAX_CONTROL_API_JSON_BODY_BYTES: usize = 64 * 1024;

#[derive(Default, Deserialize)]
pub(super) struct ControlApiRuntimePlanRequest {
    pub(super) config_path: Option<String>,
    pub(super) requested_by: Option<String>,
    pub(super) reason: Option<String>,
    pub(super) expected_generation: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct ControlApiRuntimeRollbackPayload {
    pub(super) target_generation: u64,
    pub(super) requested_by: Option<String>,
    pub(super) reason: Option<String>,
    pub(super) expected_active_generation: Option<u64>,
}

impl QUICListener {
    pub(super) async fn control_api_json_body<T>(
        req: Request<Incoming>,
    ) -> Result<T, Box<Response<Full<Bytes>>>>
    where
        T: DeserializeOwned,
    {
        let body = Self::collect_control_api_json_body_bounded(req.into_body()).await?;
        if body.is_empty() {
            return Err(Box::new(Self::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "request body is required" }),
            )));
        }
        serde_json::from_slice(&body).map_err(|err| {
            Box::new(Self::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("invalid request body: {err}") }),
            ))
        })
    }

    pub(super) async fn control_api_json_body_or_default<T>(
        req: Request<Incoming>,
    ) -> Result<T, Box<Response<Full<Bytes>>>>
    where
        T: DeserializeOwned + Default,
    {
        let body = Self::collect_control_api_json_body_bounded(req.into_body()).await?;
        if body.is_empty() {
            return Ok(T::default());
        }
        serde_json::from_slice(&body).map_err(|err| {
            Box::new(Self::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("invalid request body: {err}") }),
            ))
        })
    }

    pub(super) async fn collect_control_api_json_body_bounded<B>(
        mut body: B,
    ) -> Result<Vec<u8>, Box<Response<Full<Bytes>>>>
    where
        B: Body<Data = Bytes> + Unpin,
        B::Error: std::fmt::Display,
    {
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = match frame {
                Ok(frame) => frame,
                Err(err) => {
                    return Err(Box::new(Self::json_response(
                        StatusCode::BAD_REQUEST,
                        json!({ "error": format!("invalid request body: {err}") }),
                    )));
                }
            };
            let Ok(chunk) = frame.into_data() else {
                continue;
            };
            let next_len = bytes.len().saturating_add(chunk.len());
            if next_len > MAX_CONTROL_API_JSON_BODY_BYTES {
                return Err(Box::new(Self::json_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    json!({
                        "error": format!(
                            "request body exceeded {} bytes",
                            MAX_CONTROL_API_JSON_BODY_BYTES
                        )
                    }),
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
