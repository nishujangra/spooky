
use log::{info, error};

use super::Server;

impl Server {

pub async fn handle_connection(&self, connecting: quinn::Incoming) {
    match connecting.await {
        Ok(connection) => {
            info!("New QUIC Connection established: {}", connection.remote_address());
            self.process_connection(connection).await;
        }
        Err(e) => {
            error!("Connection failed: {}", e);
        }
    }
}
}