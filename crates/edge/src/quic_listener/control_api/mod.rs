use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use spooky_config::config::ControlApi as ControlApiConfig;
use tokio_rustls::TlsAcceptor;

use super::*;

mod audit;
mod admin_auth;
mod admin_identity;
mod context;
mod http;
mod reload;
mod render;
pub(in crate::quic_listener) mod security;
mod service;
mod state;

#[cfg(test)]
mod tests;
