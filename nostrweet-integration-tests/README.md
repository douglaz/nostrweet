# Nostrweet Integration Tests

Real integration tests for nostrweet that spin up an actual Nostr relay and verify end-to-end functionality.

## Prerequisites

- Nix development environment (provides `nostr-rs-relay`)
- Built `nostrweet` binary
- Twitter Bearer Token (for fetching real tweets)

## Building

```bash
# Build everything from workspace root
cargo build

# Or build just the integration tests
cargo build -p nostrweet-integration-tests
```

## Running Tests

### Using Just commands (recommended)

```bash
# Run all integration tests
just integration-test

# Run a specific test
just integration-test-single tweet_fetch

# Run with custom relay port
just integration-test-port 9090

# Debug mode (keeps relay running)
just integration-test-debug

# Clean up test artifacts
just integration-cleanup
```

### Direct execution

```bash
cd nostrweet-integration-tests

# Run all tests
cargo run -- run-all

# Run specific test
cargo run -- run --test tweet_fetch

# With options
cargo run -- --relay-port 9090 --verbose --keep-relay run-all

# Clean up
cargo run -- cleanup
```

## Available Tests

- **tweet_fetch**: Fetches a tweet and posts it to Nostr
- **profile**: Fetches a profile and posts metadata to Nostr
- **daemon**: Tests daemon mode automatic posting
- **nostr_post**: Tests various tweet types (replies, quotes, media)

## Test Structure

Each test:
1. Starts a fresh `nostr-rs-relay` instance with temporary database
2. Executes `nostrweet` commands via subprocess
3. Verifies results by querying the relay using `nostr-sdk`
4. Cleans up automatically

## Development

Tests are located in `src/tests/` directory. Each test module exports a single `run` function that receives a `TestContext` with:
- Relay WebSocket URL
- Temporary output directory
- Test-specific private key
- Path to nostrweet binary

## Troubleshooting

- Ensure you're in the Nix development shell: `nix develop`
- Check that `nostr-rs-relay` is available: `which nostr-rs-relay`
- Build the main binary first: `cargo build -p nostrweet`
- Use `--verbose` flag for detailed logging
- Use `--keep-relay` to inspect relay state after tests