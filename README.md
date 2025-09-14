# Spooky

A high-performance load balancer and proxy server written in Rust, supporting both HTTP/3 (QUIC) and gRPC protocols with intelligent routing and health monitoring.

## Features

- **Multi-Protocol Support**: HTTP/3 (QUIC) and gRPC
- **Load Balancing Algorithms**:
  - Weight-based balancing
  - Least connection
  - IP hash (with caching)
  - Round-robin
- **Health Monitoring**: Asynchronous health checks with configurable intervals
- **TLS Support**: Secure communication with configurable certificates
- **Dynamic Weight Updates**: Backend servers can expose `/metric` routes for weight-based balancing
- **Comprehensive Logging**: Request tracking and system monitoring

## Configuration

The system uses YAML configuration files. Here's a sample configuration:

```yaml
listen:
  protocol: http3  # or grpc
  port: 9889
  address: "0.0.0.0"
  tls:
    cer: /path/to/cert
    key: /path/to/key

backends:
  - id: "backend1"
    address: "10.0.1.100:8080"
    weight: 100  # server can have /metric route for weight-based balancing
    health_check:
      path: "/health"
      interval: 5s

load_balancing:
  type: weight-based  # weight-based, least_connection, ip-hash, round-robin
```

## Getting Started

### Prerequisites

- Rust 1.70+ 
- Apache (for serving .shtml files)

### Installation

1. Clone the repository:
```bash
git clone <repository-url>
cd spooky
```

## Configuration Options

### Listen Configuration
- `protocol`: Protocol to use (`http3` or `grpc`)
- `port`: Port number to listen on
- `address`: IP address to bind to (use `"0.0.0.0"` for all interfaces)
- `tls`: TLS configuration with certificate and key paths

### Backend Configuration
- `id`: Unique identifier for the backend
- `address`: Backend server address and port
- `weight`: Weight for load balancing (higher = more traffic)
- `health_check`: Health check configuration
  - `path`: Health check endpoint path
  - `interval`: Health check interval

### Load Balancing
- `weight-based`: Distribute traffic based on server weights
- `least_connection`: Route to server with fewest active connections
- `ip-hash`: Consistent hashing based on client IP (requires caching)
- `round-robin`: Distribute requests evenly in rotation

## License

This project is licensed under the GNU Affero General Public License v3.0 (AGPLv3). See the [LICENSE](LICENSE) file for details.

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

This project is licensed under the GNU Affero General Public License v3.0 (AGPLv3) - see the [LICENSE](LICENSE) file for details.
