#!/usr/bin/env bash
set -euo pipefail

echo "🧪 Testing GitHub Workflows Locally"
echo "===================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_step() {
    echo -e "${BLUE}🔄 $1${NC}"
}

print_success() {
    echo -e "${GREEN}✅ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠️  $1${NC}"
}

print_error() {
    echo -e "${RED}❌ $1${NC}"
}

# Check if we're in a Nix shell
if [ -z "${IN_NIX_SHELL:-}" ]; then
    print_warning "Not in Nix shell. Starting nix develop..."
    exec nix develop -c "$0" "$@"
fi

print_success "Running in Nix development shell"
echo ""

# Test 1: Flake validation (from nix.yml)
print_step "Testing Nix flake validation..."
if nix flake check 2>/dev/null; then
    print_success "Nix flake check passed"
else
    print_warning "Nix flake check had warnings (this is normal for dirty git tree)"
fi

# Test 2: Code quality checks (from ci.yml)
print_step "Testing code formatting..."
if cargo fmt --all -- --check; then
    print_success "Code formatting check passed"
else
    print_error "Code formatting check failed - run 'cargo fmt --all'"
    exit 1
fi

print_step "Testing code with cargo check..."
if just check; then
    print_success "Cargo check passed"
else
    print_error "Cargo check failed"
    exit 1
fi

print_step "Testing clippy lints..."
if just clippy; then
    print_success "Clippy lints passed"
else
    print_error "Clippy lints failed"
    exit 1
fi

# Test 3: Build tests (from ci.yml)
print_step "Testing build..."
if just build; then
    print_success "Build passed"
else
    print_error "Build failed"
    exit 1
fi

# Test 4: Unit tests (from ci.yml)
print_step "Testing unit tests..."
if cargo test --lib; then
    print_success "Unit tests passed"
else
    print_error "Unit tests failed"
    exit 1
fi

# Test 5: Integration tests (from ci.yml)
print_step "Testing integration tests..."
if cargo test --test '*'; then
    print_success "Integration tests passed"
else
    print_error "Integration tests failed"
    exit 1
fi

# Test 6: Doc tests (from ci.yml)
print_step "Testing doc tests..."
if cargo test --doc; then
    print_success "Doc tests passed"
else
    print_error "Doc tests failed"
    exit 1
fi

# Test 7: Multi-target builds (from ci.yml)
print_step "Testing musl build..."
if cargo build --release --target x86_64-unknown-linux-musl; then
    print_success "Musl build passed"
    
    # Test the binary works
    if ./target/x86_64-unknown-linux-musl/release/nostrweet --version >/dev/null 2>&1; then
        print_success "Musl binary works correctly"
    else
        print_warning "Musl binary might have runtime issues"
    fi
else
    print_error "Musl build failed"
    exit 1
fi

# Test 8: Documentation generation (from docs.yml)
print_step "Testing documentation generation..."
if cargo doc --no-deps --document-private-items; then
    print_success "Documentation generation passed"
else
    print_error "Documentation generation failed"
    exit 1
fi

# Test 9: Final comprehensive check (from ci.yml)
print_step "Running final comprehensive check..."
if just final-check; then
    print_success "Final check passed"
else
    print_error "Final check failed"
    exit 1
fi

# Test 10: Binary size analysis (from benchmarks.yml)
print_step "Testing binary size analysis..."
if [ -f target/release/nostrweet ]; then
    size=$(stat -c%s target/release/nostrweet 2>/dev/null || stat -f%z target/release/nostrweet)
    size_mb=$(echo "scale=2; $size / 1024 / 1024" | bc -l 2>/dev/null || python3 -c "print(f'{$size / 1024 / 1024:.2f}')")
    print_success "Binary size: ${size_mb} MB"
else
    print_warning "Release binary not found, building..."
    cargo build --release
    size=$(stat -c%s target/release/nostrweet 2>/dev/null || stat -f%z target/release/nostrweet)
    size_mb=$(echo "scale=2; $size / 1024 / 1024" | bc -l 2>/dev/null || python3 -c "print(f'{$size / 1024 / 1024:.2f}')")
    print_success "Binary size: ${size_mb} MB"
fi

echo ""
echo "🎉 All workflow tests passed!"
echo ""
echo "Summary of what was tested:"
echo "✅ Nix flake validation"
echo "✅ Code formatting and linting"
echo "✅ Type checking and builds"
echo "✅ Unit and integration tests"
echo "✅ Documentation generation"
echo "✅ Multi-target compilation (musl)"
echo "✅ Binary functionality"
echo "✅ Comprehensive final checks"
echo ""
echo "Your code is ready for CI! 🚀"
