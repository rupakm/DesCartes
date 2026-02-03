# DesCartes

A deterministic, replayable, discrete-event simulator for Rust.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
descartes = "0.1"
```

For all features:

```toml
[dependencies]
descartes = { version = "0.1", features = ["full"] }
```

## Features

- **Core simulation engine** (`des-core`) - Always included
- **High-level components** (`des-components`) - Included by default
- **Metrics collection** (`des-metrics`) - Included by default
- **Visualization** (`des-viz`) - Optional, enable with `viz` feature
- **Tokio integration** (`des-tokio`) - Optional, enable with `tokio` feature
- **State space exploration** (`des-explore`) - Optional, enable with `explore` feature
- **gRPC support** (`des-tonic`) - Optional, enable with `tonic` feature
- **Tower middleware** (`des-tower`) - Optional, enable with `tower` feature
- **Axum integration** (`des-axum`) - Optional, enable with `axum` feature

## Usage

```rust
use descartes::prelude::*;

// Your simulation code here
```

## Documentation

See the [repository](https://github.com/rupakm/DesCartes) for full documentation and examples.

## License

Licensed under Apache-2.0.
