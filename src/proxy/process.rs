

use log::{info, warn, debug, trace, error};

use super::Server;

use crate::lb::random::random;


impl Server {
pub async fn process_connection(&self, connection: quinn::Connection) {
    use h3::{server};
    use bytes::Bytes;
    use h3_quinn::Connection as H3QuinnConnection;

    trace!("Creating H3 connection from QUIC connection");
    let h3_connection = H3QuinnConnection::new(connection);

    let mut h3_server: h3::server::Connection<_, Bytes> = server::builder()
        .max_field_section_size(8192)
        .build(h3_connection)
        .await
        .expect("Failed to build h3 server");

    debug!("H3 server connection established, waiting for requests");

    loop {
        match h3_server.accept().await {
            Ok(Some(req_resolver)) => {
                match req_resolver.resolve_request().await {
                    Ok((req, mut _stream)) => {
                        info!("Received HTTP/3 request: {} {}", req.method(), req.uri());
                        debug!("Request headers: {:?}", req.headers());

                        // TODO: Use configured load balancing strategy instead of hardcoded random
                        // TODO: Get backends from config instead of hardcoded list
                        // TODO: Implement proper backend selection logic
                        // TODO: Add backend health status checking before selection
                        // TODO: Handle case when no healthy backends are available

                        // println!("{}", config.load_balancing.lb_type);
                        // println!("Number of backends are: {}", config.backends.len());
                        
                        if let Some(selected_backend) = random(&self.config.backends) {
                            println!("Using Backend: {}", selected_backend.address);
                        } else {
                            println!("No Selected Backend");
                        }

                        // get resposne from selected_backend 
                        // then redirect the response to lb port

                        // TODO: Implement actual request forwarding to selected backend
                        // TODO: Add HTTP/3 client connection to backend
                        // TODO: Forward request headers and body to backend
                        // TODO: Stream response back to client
                        // TODO: Handle backend connection failures
                        // TODO: Add timeout handling for backend requests
                        // TODO: Implement retry logic for failed requests
                        // TODO: Add request/response logging and metrics

                        // transfer load to that server
                        // give response

                        // // --- establish backend connection ---
                        // let backend_addr: SocketAddr = "127.0.0.1:7001".parse().unwrap();
                        // let endpoint = Endpoint::client("[::]:0".parse().unwrap()).unwrap();

                        // let quinn_conn = endpoint
                        //     .connect(backend_addr, "localhost")
                        //     .unwrap()
                        //     .await
                        //     .expect("connection to backend failed");


                        // let h3_backend = H3QuinnConnection::new(quinn_conn);
                        // let (mut h3_client, driver) = client::builder()
                        //     .max_field_section_size(8192)
                        //     .build(h3_backend)
                        //     .await
                        //     .unwrap();
                    },
                    Err(e) => {
                        error!("Error: {e:?}");
                        break;
                    }
                }
            }
            Ok(None) => {
                debug!("Connection closed gracefully");
                break;
            }
            Err(e) => {
                warn!("Connection closed with error: {e:?}");
                break;
            }
        }
    }
}
 
}