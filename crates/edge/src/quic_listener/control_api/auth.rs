use ::http::{Method, header};
use bytes::Bytes;
use http_body_util::Full;
use subtle::ConstantTimeEq;

use super::{
    state::{ControlApiPaths, ControlApiState},
    *,
};

type ControlApiGateError = Box<Response<Full<Bytes>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControlApiRoute {
    Health,
    Ready,
    Runtime,
    RuntimeValidate,
    RuntimePreview,
    RuntimeActivate,
    RuntimeRollback,
    RuntimeHistory,
    RuntimeHistoryGeneration(u64),
    ReloadCerts,
    ReloadRuntime,
    Restart,
}

impl ControlApiRoute {
    fn requires_authorization(self) -> bool {
        !matches!(self, Self::Health | Self::Ready)
    }
}

impl QUICListener {
    pub(super) fn control_api_request_route_for<B>(
        req: &::http::Request<B>,
        paths: &ControlApiPaths,
    ) -> Option<ControlApiRoute> {
        let path = req.uri().path();
        let runtime_validate_path = paths.runtime_validate_path();
        let runtime_preview_path = paths.runtime_preview_path();
        let runtime_activate_path = paths.runtime_activate_path();
        let runtime_rollback_path = paths.runtime_rollback_path();
        let runtime_history_path = paths.runtime_history_path();
        let runtime_history_entry_prefix = paths.runtime_history_entry_prefix();

        match *req.method() {
            Method::GET if path == paths.health_path.as_str() => Some(ControlApiRoute::Health),
            Method::GET if path == paths.ready_path.as_str() => Some(ControlApiRoute::Ready),
            Method::GET if path == paths.runtime_path.as_str() => Some(ControlApiRoute::Runtime),
            Method::GET if path == runtime_history_path.as_str() => {
                Some(ControlApiRoute::RuntimeHistory)
            }
            Method::GET if path.starts_with(runtime_history_entry_prefix.as_str()) => path
                .strip_prefix(runtime_history_entry_prefix.as_str())
                .and_then(|raw_generation| raw_generation.parse::<u64>().ok())
                .map(ControlApiRoute::RuntimeHistoryGeneration),
            Method::POST if path == paths.reload_certs_path.as_str() => {
                Some(ControlApiRoute::ReloadCerts)
            }
            Method::POST if path == runtime_validate_path.as_str() => {
                Some(ControlApiRoute::RuntimeValidate)
            }
            Method::POST if path == runtime_preview_path.as_str() => {
                Some(ControlApiRoute::RuntimePreview)
            }
            Method::POST if path == runtime_activate_path.as_str() => {
                Some(ControlApiRoute::RuntimeActivate)
            }
            Method::POST if path == runtime_rollback_path.as_str() => {
                Some(ControlApiRoute::RuntimeRollback)
            }
            Method::POST if path == paths.reload_path.as_str() => {
                Some(ControlApiRoute::ReloadRuntime)
            }
            Method::POST if path == paths.restart_path.as_str() => Some(ControlApiRoute::Restart),
            _ => None,
        }
    }

    pub(super) fn bearer_token_from_authorization_header(raw: &str) -> Option<&str> {
        let raw = raw.trim();
        let split = raw.find(char::is_whitespace)?;
        let (scheme, rest) = raw.split_at(split);
        if !scheme.eq_ignore_ascii_case("bearer") {
            return None;
        }
        let token = rest.trim_start();
        if token.is_empty() {
            return None;
        }
        Some(token)
    }

    pub(super) fn control_api_is_authorized_for<B>(
        req: &::http::Request<B>,
        endpoint: &ControlApiConfig,
    ) -> bool {
        let Some(token) = endpoint.auth_token.as_ref() else {
            return false;
        };
        let Some(header) = req.headers().get(header::AUTHORIZATION) else {
            return false;
        };
        let Ok(raw) = header.to_str() else {
            return false;
        };
        let Some(provided) = Self::bearer_token_from_authorization_header(raw) else {
            return false;
        };
        bool::from(provided.as_bytes().ct_eq(token.as_bytes()))
    }

    pub(super) fn authorize_control_api_request_for<B>(
        req: &::http::Request<B>,
        state: &ControlApiState,
        route: ControlApiRoute,
    ) -> Result<(), ControlApiGateError> {
        let service_state = state.current_service_state();
        if !route.requires_authorization()
            || Self::control_api_is_authorized_for(req, &service_state.endpoint)
        {
            return Ok(());
        }

        let response = match route {
            ControlApiRoute::Runtime
            | ControlApiRoute::RuntimeValidate
            | ControlApiRoute::RuntimePreview
            | ControlApiRoute::RuntimeActivate
            | ControlApiRoute::RuntimeRollback
            | ControlApiRoute::RuntimeHistory
            | ControlApiRoute::RuntimeHistoryGeneration(_) => json!({
                "error": "unauthorized",
            }),
            ControlApiRoute::ReloadCerts | ControlApiRoute::ReloadRuntime => json!({
                "reloaded": false,
                "error": "unauthorized",
            }),
            ControlApiRoute::Restart => json!({
                "accepted": false,
                "error": "unauthorized",
            }),
            ControlApiRoute::Health | ControlApiRoute::Ready => {
                debug_assert!(
                    false,
                    "unauthenticated route {route:?} reached the authorization-rejection path"
                );
                json!({
                    "error": "unauthorized",
                })
            }
        };
        Err(Box::new(Self::json_response(
            StatusCode::UNAUTHORIZED,
            response,
        )))
    }

    pub(super) fn gate_control_api_request_for<B>(
        req: &::http::Request<B>,
        state: &ControlApiState,
    ) -> Result<ControlApiRoute, ControlApiGateError> {
        let service_state = state.current_service_state();
        let Some(route) = Self::control_api_request_route_for(req, &service_state.paths) else {
            return Err(Box::new(Self::control_api_not_found_response()));
        };
        Self::authorize_control_api_request_for(req, state, route)?;
        Ok(route)
    }

    pub(super) fn gate_control_api_request(
        req: &Request<Incoming>,
        state: &ControlApiState,
    ) -> Result<ControlApiRoute, ControlApiGateError> {
        Self::gate_control_api_request_for(req, state)
    }

    pub(super) fn control_api_not_found_response() -> Response<Full<Bytes>> {
        match Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"not found\n")))
        {
            Ok(resp) => resp,
            Err(_) => Response::new(Full::new(Bytes::from_static(b"not found\n"))),
        }
    }
}
