use ::http::{Method, header};
use bytes::Bytes;
use http_body_util::Full;
use subtle::ConstantTimeEq;

use super::{state::ControlApiState, *};

type ControlApiGateError = Box<Response<Full<Bytes>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControlApiRoute {
    Health,
    Ready,
    Runtime,
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

    pub(super) fn control_api_request_route(
        req: &Request<Incoming>,
        state: &ControlApiState,
    ) -> Option<ControlApiRoute> {
        let state = state.current_service_state();
        let paths = state.paths;
        let path = req.uri().path();

        match *req.method() {
            Method::GET if path == paths.health_path.as_str() => Some(ControlApiRoute::Health),
            Method::GET if path == paths.ready_path.as_str() => Some(ControlApiRoute::Ready),
            Method::GET if path == paths.runtime_path.as_str() => Some(ControlApiRoute::Runtime),
            Method::POST if path == paths.reload_certs_path.as_str() => {
                Some(ControlApiRoute::ReloadCerts)
            }
            Method::POST if path == paths.reload_path.as_str() => {
                Some(ControlApiRoute::ReloadRuntime)
            }
            Method::POST if path == paths.restart_path.as_str() => Some(ControlApiRoute::Restart),
            _ => None,
        }
    }

    pub(super) fn authorize_control_api_request(
        req: &Request<Incoming>,
        state: &ControlApiState,
        route: ControlApiRoute,
    ) -> Result<(), ControlApiGateError> {
        if !route.requires_authorization() || Self::control_api_is_authorized(req, state) {
            return Ok(());
        }

        let response = match route {
            ControlApiRoute::Runtime => json!({
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
            // Health/Ready return `false` from `requires_authorization()`, so the
            // early return above already handled them and this arm is unreachable.
            // Phase 5: rather than `unreachable!()` (a production panic if that
            // invariant is ever changed), fail closed with a generic unauthorized
            // body and assert the invariant only in debug/test builds.
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

    pub(super) fn gate_control_api_request(
        req: &Request<Incoming>,
        state: &ControlApiState,
    ) -> Result<ControlApiRoute, ControlApiGateError> {
        let Some(route) = Self::control_api_request_route(req, state) else {
            return Err(Box::new(Self::control_api_not_found_response()));
        };
        Self::authorize_control_api_request(req, state, route)?;
        Ok(route)
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

    pub(super) fn control_api_is_authorized(
        req: &Request<Incoming>,
        state: &ControlApiState,
    ) -> bool {
        let endpoint = state.current_service_state().endpoint;
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
}
