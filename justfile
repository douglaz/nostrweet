# Justfile for Nostrweet
# Simple build and development commands
#
# Usage examples:
#   just build        # Build the project
#   just test         # Run all tests
#   just pre-commit   # Run all quality checks
#   just build-musl   # Build static binary

# Default recipe - show available commands
default:
    @just --list

# Build the project
build:
    cargo build

# Build for release
build-release:
    cargo build --release

# Run all tests
test:
    cargo test

# Check code without building
check:
    cargo check --workspace --all-targets

# Run clippy lints
clippy:
    cargo clippy --workspace --all-targets --all-features -- --deny warnings

# Fix clippy warnings automatically
clippy-fix:
    cargo clippy --workspace --all-targets --all-features --fix

# Format all code
format:
    cargo fmt --all

# Check if code is formatted
format-check:
    cargo fmt --all -- --check

# Build for musl target (static binary)
build-musl:
    cargo build --release --target x86_64-unknown-linux-musl

# Run the application with arguments
run *ARGS:
    cargo run -- {{ARGS}}

# Watch and rebuild on changes
watch:
    cargo watch -x check -x test -x run

# Clean build artifacts
clean:
    cargo clean

# Install the binary locally
install:
    cargo install --path .

# Run comprehensive checks before PR
pre-commit: format format-check clippy test
    @echo "✅ All checks passed!"

# Alias for pre-commit (used in CI)
final-check: pre-commit

# Check for typos (placeholder - requires typos tool)
typos:
    @echo "⚠️  Typos check not configured. Install 'typos' tool if needed."

# Update dependencies
update:
    cargo update

# Build integration tests
build-integration-tests:
    cargo build -p nostrweet-integration-tests

# Run all integration tests
integration-test: build
    cd nostrweet-integration-tests && cargo run -- run-all

# Run a specific integration test
integration-test-single TEST: build
    cd nostrweet-integration-tests && cargo run -- run --test {{TEST}}

# Run integration tests with custom relay port
integration-test-port PORT: build
    cd nostrweet-integration-tests && cargo run -- --relay-port {{PORT}} run-all

# Run integration tests keeping relay running
integration-test-debug: build
    cd nostrweet-integration-tests && cargo run -- --keep-relay --verbose run-all

# Clean up integration test artifacts
integration-cleanup:
    cd nostrweet-integration-tests && cargo run -- cleanup