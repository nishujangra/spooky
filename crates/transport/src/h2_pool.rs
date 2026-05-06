use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};

use http_body_util::combinators::BoxBody;
use hyper::Request;
use hyper::body::{Bytes, Incoming};
use tokio::sync::{Semaphore, TryAcquireError};

use crate::h2_client::{H2Client, TlsClientConfig};
pub use spooky_errors::PoolError;

struct BackendHandle {
    client: H2Client,
    inflight: Arc<Semaphore>,
}

pub struct H2Pool {
    backend_index: HashMap<String, usize>,
    backends: Vec<BackendHandle>,
}

impl H2Pool {
    pub fn new<I>(
        backends: I,
        max_inflight: usize,
        max_idle_per_backend: usize,
        pool_idle_timeout: Duration,
        connect_timeout: Duration,
        tls: TlsClientConfig,
    ) -> Result<Self, String>
    where
        I: IntoIterator<Item = String>,
    {
        let inflight = max_inflight.max(1);
        let max_idle_per_backend = max_idle_per_backend.max(1);
        let mut index = HashMap::new();
        let mut backend_handles = Vec::new();
        for backend in backends {
            let client = H2Client::new(
                max_idle_per_backend,
                pool_idle_timeout,
                connect_timeout,
                tls.clone(),
            )?;
            let slot = backend_handles.len();
            index.insert(backend, slot);
            backend_handles.push(BackendHandle {
                client,
                inflight: Arc::new(Semaphore::new(inflight)),
            });
        }
        Ok(Self {
            backend_index: index,
            backends: backend_handles,
        })
    }

    pub fn has_backend(&self, backend: &str) -> bool {
        self.backend_index.contains_key(backend)
    }

    pub fn backend_index(&self, backend: &str) -> Option<usize> {
        self.backend_index.get(backend).copied()
    }

    pub async fn send(
        &self,
        backend: &str,
        req: Request<BoxBody<Bytes, Infallible>>,
    ) -> Result<hyper::Response<Incoming>, PoolError> {
        let idx = self
            .backend_index(backend)
            .ok_or_else(|| PoolError::UnknownBackend(backend.to_string()))?;
        self.send_by_index(idx, req).await
    }

    pub async fn send_by_index(
        &self,
        backend_index: usize,
        req: Request<BoxBody<Bytes, Infallible>>,
    ) -> Result<hyper::Response<Incoming>, PoolError> {
        let handle = self
            .backends
            .get(backend_index)
            .ok_or_else(|| PoolError::UnknownBackend(backend_index.to_string()))?;
        let _permit = match Arc::clone(&handle.inflight).try_acquire_owned() {
            Ok(permit) => permit,
            Err(TryAcquireError::NoPermits) => {
                return Err(PoolError::BackendOverloaded(format!(
                    "backend-index:{}",
                    backend_index
                )));
            }
            Err(TryAcquireError::Closed) => return Err(PoolError::InflightLimiterClosed),
        };
        handle.client.send(req).await.map_err(PoolError::Send)
    }
}
