use std::{convert::Infallible, net::SocketAddr};

use bytes::Bytes;
use clap::Parser;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(version, about = "Minimal HTTP/2 backend for Impulse")]
struct Cli {
    #[arg(long, default_value_t = 8081)]
    port: u16,
}

async fn handle_request(_req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    Ok(Response::new(Full::new(Bytes::from("backend ok\n"))))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    let addr: SocketAddr = format!("127.0.0.1:{}", cli.port).parse()?;

    let listener = TcpListener::bind(addr).await?;
    println!("HTTP/2 backend listening on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let service = service_fn(handle_request);

        tokio::spawn(async move {
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(io, service)
                .await;
        });
    }
}
