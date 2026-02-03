# Publishing Guide for DesCartes

This guide explains how to publish the DesCartes workspace to crates.io.

## Overview

The workspace uses a "meta-crate" pattern where:
- Individual crates (`des-core`, `des-components`, etc.) are published separately
- The `descartes` crate re-exports all of them with feature flags
- Users only need to depend on `descartes` in their `Cargo.toml`

## Publishing Order

**IMPORTANT**: Publish in dependency order to avoid errors.

### 1. Core Dependencies (no internal dependencies)
```bash
cargo publish -p des-core
```

### 2. First-tier Dependencies (depend only on des-core)
```bash
cargo publish -p des-metrics
cargo publish -p des-tokio
```

### 3. Second-tier Dependencies
```bash
cargo publish -p des-components  # depends on des-core, des-tokio
cargo publish -p des-explore     # depends on des-core, des-tokio
cargo publish -p des-tower       # depends on des-core, des-tokio
```

### 4. Third-tier Dependencies
```bash
cargo publish -p des-tonic-build  # build dependency for des-tonic
cargo publish -p des-tonic        # depends on des-core, des-tokio
cargo publish -p des-axum         # depends on des-core, des-tokio
cargo publish -p des-viz          # depends on des-metrics
```

### 5. Meta-crate (depends on all others)
```bash
cargo publish -p descartes
```

## Pre-publish Checklist

Before publishing, ensure:

1. **Version numbers are consistent** across all crates (currently 0.1.1)
2. **All tests pass**:
   ```bash
   cargo test --workspace
   ```
3. **Clippy is clean**:
   ```bash
   cargo clippy --workspace --all-features
   ```
4. **Documentation builds**:
   ```bash
   cargo doc --workspace --all-features --no-deps
   ```
5. **README files are up to date** in each crate
6. **License files are present** (Apache-2.0)

## Dry Run

Test the publishing process without actually uploading:

```bash
cargo publish -p des-core --dry-run
cargo publish -p descartes --dry-run
```

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
2. Update dependency versions in `descartes/Cargo.toml` to match
3. Follow the same publishing order as above

## Troubleshooting

**Error: "crate not found"**
- You're trying to publish a crate before its dependencies
- Follow the publishing order above

**Error: "version already exists"**
- You've already published this version
- Bump the version number and try again

**Error: "missing documentation"**
- Add doc comments to public items
- Or add `#![allow(missing_docs)]` to lib.rs (not recommended)
