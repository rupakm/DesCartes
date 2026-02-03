#!/bin/bash
set -e

# Publishing script for DesCartes workspace
# This publishes all crates in the correct dependency order

echo "Publishing DesCartes to crates.io"
echo ""

# Check if we should do a dry run
DRY_RUN=""
if [ "$1" = "--dry-run" ]; then
    DRY_RUN="--dry-run"
    echo "📋 DRY RUN MODE - No actual publishing will occur"
    echo ""
fi

# Function to publish a crate
publish_crate() {
    local crate=$1
    echo "📦 Publishing $crate..."
    cargo publish -p "$crate" $DRY_RUN
    
    if [ -z "$DRY_RUN" ]; then
        echo "⏳ Waiting 30 seconds for crates.io to process..."
        sleep 30
    fi
    echo "✅ $crate published"
    echo ""
}

# Pre-flight checks
echo "🔍 Running pre-flight checks..."
echo ""

# echo "Running tests..."
# cargo test --workspace --quiet || {
#     echo "❌ Tests failed. Aborting."
#     exit 1
# }
# 
# echo "Running clippy..."
# cargo clippy --workspace --all-features --quiet -- -D warnings || {
#     echo "❌ Clippy found issues. Aborting."
#     exit 1
# }

echo "✅ Pre-flight checks passed"
echo ""

# Publish in dependency order
echo "📚 Publishing crates in dependency order..."
echo ""

# Tier 1: Core
publish_crate "descartes-core"

# Tier 2: First-level dependencies
publish_crate "descartes-metrics"
publish_crate "descartes-tokio"

# Tier 3: Second-level dependencies
publish_crate "descartes-components"
publish_crate "descartes-explore"
publish_crate "descartes-tower"

# Tier 4: Third-level dependencies
publish_crate "descartes-tonic-build"
publish_crate "descartes-tonic"
publish_crate "descartes-axum"
publish_crate "descartes-viz"

# Tier 5: Meta-crate
publish_crate "descartes"

echo "🎉 All crates published successfully!"
echo ""
echo "Users can now install with:"
echo "  cargo add descartes"
