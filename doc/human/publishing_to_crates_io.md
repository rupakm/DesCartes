# Publishing Guide for DesCartes

This guide explains how to publish the DesCartes workspace to crates.io.

## Overview

The workspace uses a "meta-crate" pattern where:
- Individual crates (`descartes-core`, `descartes-components`, etc.) are published separately
- The `descartes` crate re-exports all of them with feature flags
- Users only need to depend on `descartes` in their `Cargo.toml`

## Pre-publish Checklist

Before publishing, ensure:

1. **Version numbers are consistent** across all crates (currently 0.1.1)
2. **All internal dependencies have explicit versions** (e.g., `descartes-core = { version = "0.1.1", path = "../des-core" }`)
3. **Circular dev-dependencies are handled**:
   - `descartes-core` has `descartes-explore` as a dev-dependency (circular)
   - Either temporarily comment it out for initial publish, or accept that the example won't compile until explore is published
4. **All tests pass**:
   ```bash
   cargo test --workspace
   ```
5. **Clippy is clean**:
   ```bash
   cargo clippy --workspace --all-features
   ```
6. **Documentation builds**:
   ```bash
   cargo doc --workspace --all-features --no-deps
   ```
7. **README files are up to date** in each crate
8. **License files are present** (Apache-2.0)

## Publishing Order

**IMPORTANT**: Publish in dependency order to avoid errors.

### 1. Core Dependencies (no internal dependencies)
```bash
# Temporarily comment out descartes-explore dev-dependency in des-core/Cargo.toml
cargo publish -p descartes-core
```

### 2. First-tier Dependencies (depend only on descartes-core)
```bash
cargo publish -p descartes-metrics
cargo publish -p descartes-tokio
```

### 3. Second-tier Dependencies
```bash
cargo publish -p descartes-components  # depends on descartes-core, descartes-tokio, descartes-metrics
cargo publish -p descartes-explore     # depends on descartes-core, descartes-tokio
cargo publish -p descartes-tower       # depends on descartes-core, descartes-tokio, descartes-components
```

### 4. Third-tier Dependencies
```bash
cargo publish -p descartes-tonic-build  # build dependency for descartes-tonic
cargo publish -p descartes-tonic        # depends on descartes-core, descartes-tokio, descartes-components, descartes-tower
cargo publish -p descartes-axum         # depends on descartes-core, descartes-tokio, descartes-components
cargo publish -p descartes-viz          # depends on descartes-core, descartes-metrics
```

### 5. Meta-crate (depends on all others)
```bash
cargo publish -p descartes
```

### 6. Post-publish Cleanup
```bash
# Uncomment descartes-explore in des-core/Cargo.toml dev-dependencies
# Commit the change
```

## Dry Run

Test the publishing process without actually uploading:

```bash
cargo publish -p descartes-core --dry-run --allow-dirty
cargo publish -p descartes --dry-run --allow-dirty
```

Note: `--allow-dirty` is needed if you have uncommitted changes.

## User Experience

After publishing, users can simply add to their `Cargo.toml`:

```toml
# Minimal (core + components + metrics)
[dependencies]
descartes = "0.1"

# With specific features
[dependencies]
descartes = { version = "0.1", features = ["tokio", "explore"] }

# Everything
[dependencies]
descartes = { version = "0.1", features = ["full"] }
```

Then in their code:
```rust
use descartes::prelude::*;

fn main() {
    let mut sim = Simulation::default();
    // ...
}
```

## Version Updates

When bumping versions:

1. Update `version` in `Cargo.toml` workspace.package section
2. Update dependency versions in all `Cargo.toml` files to match (e.g., `descartes-core = { version = "0.1.2", path = "../des-core" }`)
3. Follow the same publishing order as above

## Troubleshooting

**Error: "no matching package named `descartes-core` found"**
- You're trying to publish a crate before its dependencies
- Follow the publishing order above
- For circular dev-dependencies, temporarily comment them out

**Error: "all dependencies must have a version specified"**
- Add explicit version to internal dependencies: `descartes-core = { version = "0.1.1", path = "../des-core" }`

**Error: "version already exists"**
- You've already published this version
- Bump the version number and try again

**Error: "files in the working directory contain changes"**
- Commit your changes first, or use `--allow-dirty` flag for testing
