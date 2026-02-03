# DesCartes Meta-Crate Design

## Problem

The DesCartes project is organized as a Cargo workspace with multiple crates:
- `des-core` - Core simulation engine
- `des-components` - High-level components
- `des-metrics` - Metrics collection
- `des-viz` - Visualization
- `des-tokio` - Tokio integration
- `des-explore` - State space exploration
- `des-tonic` - gRPC support
- `des-tower` - Tower middleware
- `des-axum` - Axum integration

Users would need to manually add multiple dependencies to use the framework.

## Solution

Create a **meta-crate** (`descartes`) that:
1. Re-exports all workspace crates
2. Uses feature flags for optional dependencies
3. Provides a unified `prelude` module with common imports
4. Gives users a single dependency to add

## Architecture

### Feature Flags

- `default = ["components", "metrics"]` - Most common use case
- `full` - Everything enabled
- Individual features for each crate (`tokio`, `explore`, `tonic`, etc.)
- Convenience bundles (`testing`, `networking`)

### Re-exports

```rust
pub use des_core as core;
pub use des_components as components;  // feature-gated
// ... etc
```

### Prelude Module

Common types re-exported for convenience:
```rust
use descartes::prelude::*;
// Gets: Simulation, SimTime, Executor, Component, etc.
```

## Publishing Process

1. Publish all workspace crates individually to crates.io (in dependency order)
2. Publish the `descartes` meta-crate last
3. Users only need to depend on `descartes`

## User Experience

### Before (without meta-crate)
```toml
[dependencies]
des-core = "0.1"
des-components = "0.1"
des-metrics = "0.1"
des-tokio = "0.1"
```

### After (with meta-crate)
```toml
[dependencies]
descartes = { version = "0.1", features = ["tokio"] }
```

## Examples

See `examples/basic_usage.rs` for a minimal example.

## References

This pattern is used by:
- `tokio` (re-exports tokio-util, tokio-stream, etc.)
- `aws-sdk-rust` (re-exports individual service crates)
- `bevy` (re-exports bevy_ecs, bevy_render, etc.)
