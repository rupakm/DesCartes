#!/bin/bash
set -e

# Script to run all examples in all workspace packages
# This helps verify that all examples compile and run successfully
#
# NOTE: Many simulation examples run for extended periods or indefinitely.
# The timeout ensures the script completes. Examples that timeout are not
# necessarily broken - they may just be long-running simulations.
#
# Usage:
#   ./scripts/run_all_examples.sh              # Run with 30s timeout
#   ./scripts/run_all_examples.sh --verbose    # Show output from examples
#   ./scripts/run_all_examples.sh --timeout 60 # Custom timeout in seconds

# Parse arguments
VERBOSE=false
TIMEOUT=30

while [[ $# -gt 0 ]]; do
    case $1 in
        --verbose|-v)
            VERBOSE=true
            shift
            ;;
        --timeout|-t)
            TIMEOUT="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --verbose, -v       Show output from examples"
            echo "  --timeout, -t SEC   Set timeout in seconds (default: 30)"
            echo "  --help, -h          Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

echo "🚀 Running all examples in workspace packages"
echo "⏱️  Timeout: ${TIMEOUT}s per example"
if [ "$VERBOSE" = true ]; then
    echo "📢 Verbose mode enabled"
fi
echo ""

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Counters
TOTAL_EXAMPLES=0
PASSED_EXAMPLES=0
FAILED_EXAMPLES=0

# Array to track failures
declare -a FAILED_LIST

# Function to get package name from directory
get_package_name() {
    local dir=$1
    local cargo_toml="${dir}/Cargo.toml"
    
    if [ ! -f "$cargo_toml" ]; then
        echo ""
        return
    fi
    
    # Extract package name from Cargo.toml
    grep -m 1 '^name = ' "$cargo_toml" | sed 's/name = "\(.*\)"/\1/'
}

# Function to run examples for a package
run_package_examples() {
    local package_dir=$1
    local package_name=$(get_package_name "$package_dir")
    local examples_dir="${package_dir}/examples"
    
    if [ -z "$package_name" ]; then
        echo "  ⚠️  Could not determine package name"
        return
    fi
    
    # Check if examples directory exists
    if [ ! -d "$examples_dir" ]; then
        echo "  ℹ️  No examples directory found"
        return
    fi
    
    # Find all .rs files in examples directory
    local example_files=$(find "$examples_dir" -maxdepth 1 -name "*.rs" -type f)
    
    if [ -z "$example_files" ]; then
        echo "  ℹ️  No example files found"
        return
    fi
    
    # Run each example
    while IFS= read -r example_file; do
        local example_name=$(basename "$example_file" .rs)
        TOTAL_EXAMPLES=$((TOTAL_EXAMPLES + 1))
        
        echo -n "  📝 Running example: $example_name ... "
        
        # Run the example with a timeout
        if [ "$VERBOSE" = true ]; then
            echo "" # New line before output
            echo "    Command: cargo run -p $package_name --example $example_name"
            if timeout "${TIMEOUT}s" cargo run -p "$package_name" --example "$example_name" 2>&1; then
                echo -e "${GREEN}✓ Passed${NC}"
                PASSED_EXAMPLES=$((PASSED_EXAMPLES + 1))
            else
                local exit_code=$?
                if [ $exit_code -eq 124 ]; then
                    echo -e "${RED}✗ Timeout (${TIMEOUT}s)${NC}"
                else
                    echo -e "${RED}✗ Failed (exit code: $exit_code)${NC}"
                fi
                FAILED_EXAMPLES=$((FAILED_EXAMPLES + 1))
                FAILED_LIST+=("$package_name::$example_name")
            fi
        else
            if timeout "${TIMEOUT}s" cargo run -p "$package_name" --example "$example_name" > /dev/null 2>&1; then
                echo -e "${GREEN}✓${NC}"
                PASSED_EXAMPLES=$((PASSED_EXAMPLES + 1))
            else
                local exit_code=$?
                if [ $exit_code -eq 124 ]; then
                    echo -e "${RED}✗ (timeout)${NC}"
                else
                    echo -e "${RED}✗ (exit: $exit_code)${NC}"
                fi
                FAILED_EXAMPLES=$((FAILED_EXAMPLES + 1))
                FAILED_LIST+=("$package_name::$example_name")
            fi
        fi
    done <<< "$example_files"
}

# List of package directories to check (in workspace order)
PACKAGE_DIRS=(
    "descartes"
    "des-core"
    "des-components"
    "des-metrics"
    "des-viz"
    "des-tokio"
    "des-explore"
    "des-tonic"
    "des-tower"
    "des-axum"
)

# Run examples for each package
for package_dir in "${PACKAGE_DIRS[@]}"; do
    if [ -d "$package_dir" ]; then
        package_name=$(get_package_name "$package_dir")
        echo "📦 Package: $package_dir (cargo package: $package_name)"
        run_package_examples "$package_dir"
        echo ""
    fi
done

# Print summary
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Total examples:  $TOTAL_EXAMPLES"
echo -e "Passed:          ${GREEN}$PASSED_EXAMPLES${NC}"
echo -e "Failed:          ${RED}$FAILED_EXAMPLES${NC}"
echo ""

# Print failed examples if any
if [ $FAILED_EXAMPLES -gt 0 ]; then
    echo -e "${RED}Failed examples:${NC}"
    for failed in "${FAILED_LIST[@]}"; do
        echo "  ❌ $failed"
    done
    echo ""
    exit 1
else
    echo -e "${GREEN}🎉 All examples passed!${NC}"
    exit 0
fi
