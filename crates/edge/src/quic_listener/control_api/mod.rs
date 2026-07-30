use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use spooky_config::config::ControlApi as ControlApiConfig;
use tokio_rustls::TlsAcceptor;

use super::*;

mod auth;
mod context;
mod http;
mod reload;
mod render;
mod service;
mod state;

#[cfg(test)]
mod tests;
