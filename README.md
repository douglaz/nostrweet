# Nostrweet

[![CI](https://github.com/douglaz/nostrweet/actions/workflows/ci.yml/badge.svg)](https://github.com/douglaz/nostrweet/actions/workflows/ci.yml)
[![Integration Tests](https://github.com/douglaz/nostrweet/actions/workflows/integration-tests.yml/badge.svg)](https://github.com/douglaz/nostrweet/actions/workflows/integration-tests.yml)

A Rust CLI tool that downloads tweets with their media from Twitter and seamlessly publishes them to Nostr relays. Combines Twitter archiving functionality with Nostr social media publishing, enabling users to migrate their Twitter content to decentralized protocols.

## Key Features

### Twitter Integration
- Download individual tweets by URL or ID with all associated media
- Fetch multiple tweets from user timelines with smart pagination
- Download user profiles with comprehensive metadata
- Support for all media types (images, videos, GIFs)
- Smart caching system to avoid duplicate downloads
- Handle retweets, replies, and quoted tweets with full context
- High-quality media downloads with automatic format selection

### Nostr Integration
- Convert tweets to Nostr events with proper formatting
- Post tweets and profiles to multiple Nostr relays simultaneously
- Upload media to Blossom servers for decentralized storage
- Support for referenced tweets with media URL expansion
- Deterministic event IDs for consistency
- Relay list management and updates
- Private key management with secure key generation

### Data Management
- Efficient local caching with deduplication
- JSON storage for all tweet metadata
- Media file organization with descriptive naming
- Profile caching and incremental updates
- Comprehensive logging and error handling

## Requirements

- **Rust 2021 edition**
- **Twitter API Bearer Token** (set as `TWITTER_BEARER_TOKEN` environment variable)
- **Nostr Private Key** (optional, can be auto-generated)

## Installation

```bash
git clone https://github.com/yourusername/nostrweet.git
cd nostrweet
cargo build --release
```

## Environment Setup

Create a `.env` file in the project root:

```bash
TWITTER_BEARER_TOKEN=your_twitter_bearer_token
RUST_LOG=info  # Optional: debug, trace for more verbose output
NOSTRWEET_DATA_DIR=./downloads  # Optional: data directory for all storage
```

## Usage

### Global Options
- `-o, --data-dir <DIR>`: Specify the directory to save all data (tweets, media, profiles) (default: `./downloads`)
- `-v, --verbose`: Enable verbose output logging
- `-h, --help`: Display help information
- `-V, --version`: Show version information

## Commands

### Twitter Operations

#### Fetch Individual Tweet
```bash
# Download by URL
nostrweet fetch-tweet https://twitter.com/username/status/1234567890

# Download by ID
nostrweet fetch-tweet 1234567890

# Custom output directory
nostrweet --data-dir /path/to/downloads fetch-tweet 1234567890
```

#### Fetch User Timeline
```bash
# Download recent tweets (default: 10)
nostrweet user-tweets username

# Download specific number of tweets
nostrweet user-tweets --count 50 username

# Download tweets from last 7 days only
nostrweet user-tweets --days 7 username
```

#### Fetch User Profile
```bash
nostrweet fetch-profile username
```

#### List Downloaded Content
```bash
nostrweet list-tweets
```

#### Clear Cache
```bash
# With confirmation prompt
nostrweet clear-cache

# Force clear without confirmation
nostrweet clear-cache --force
```

### Nostr Operations

#### Post Single Tweet to Nostr
```bash
nostrweet post-tweet-to-nostr 1234567890 \
  --relays ws://relay1.example.com,wss://relay2.example.com \
  --blossom-servers https://blossom1.example.com \
  --private-key your_hex_private_key
```

#### Post All User Tweets to Nostr
```bash
nostrweet post-user-to-nostr username \
  --relays wss://relay.example.com \
  --blossom-servers https://blossom.example.com \
  --force  # Overwrite existing events
```

#### Post User Profile to Nostr
```bash
nostrweet post-profile-to-nostr username \
  --relays wss://relay.example.com \
  --private-key your_hex_private_key
```

#### Update Relay List
```bash
nostrweet update-relay-list \
  --relays wss://relay1.example.com,wss://relay2.example.com \
  --private-key your_hex_private_key
```

#### Show Tweet as Nostr Event
```bash
# Preview how a tweet will look as a Nostr event
nostrweet show-tweet 1234567890
```

## Architecture & Technical Details

### Code Organization
- **Commands** (`src/commands/`): All CLI command implementations
- **Twitter API** (`src/twitter.rs`): Twitter client with comprehensive data structures
- **Nostr Integration** (`src/nostr.rs`): Event formatting and relay publishing
- **Media Handling** (`src/media.rs`): Download and URL extraction logic
- **Storage** (`src/storage.rs`): Local caching and file management
- **Key Management** (`src/keys.rs`): Nostr private key handling

### Data Formats

#### Tweet Storage
```
downloads/
├── 20240315_143022_username_1234567890.json    # Tweet metadata
├── username_1234567890_0.jpg                   # First image
├── username_1234567890_1.mp4                   # Video file
└── nostr_events/
    └── abc123...def456.json                     # Generated Nostr event
```

#### Nostr Event Format
Tweets are converted to Nostr events with:
- **Kind 1**: Text notes (standard Nostr posts)
- **Proper formatting**: Tweet author, content, and media URLs
- **Referenced content**: Replies and quotes with full context
- **Media URLs**: Direct links to images/videos (not Twitter page links)
- **Tags**: Reference URLs and client identification

### Quality Assurance

#### Testing
- **70 unit tests** covering core functionality
- **22 integration tests** with real tweet data
- **Regression tests** for critical formatting scenarios
- **Pretty assertions** for detailed test failure output

#### Code Quality
- **Clippy compliance** with strict warning denial
- **Consistent formatting** with rustfmt
- **Comprehensive error handling** using anyhow
- **No unwrap/panic** usage in production code

## Media URL Expansion

The tool properly expands Twitter's shortened URLs (`t.co` links) to actual media URLs:

- **Before**: `pic.x.com/AbC123` → Twitter page link
- **After**: `https://pbs.twimg.com/media/actual_image.jpg` → Direct media URL

This ensures Nostr events contain directly accessible media links.

## Error Handling & Reliability

- **Rate limit handling**: Automatic backoff and retry logic
- **Network resilience**: Timeout handling and connection recovery
- **Data validation**: Comprehensive input validation and error messages
- **Graceful degradation**: Continue processing when individual items fail
- **Detailed logging**: Comprehensive tracing for debugging

## Development

### Setup

```bash
# Enter development environment (includes automatic Git hook setup)
nix develop

# Or manually install hooks (for non-nix users)
git config core.hooksPath .githooks
```

### Building
```bash
# Development build
cargo build

# Release build
cargo build --release

# Watch and rebuild on changes
cargo watch -x run
```

### Testing
```bash
# Run all tests
cargo test

# Run specific test module
cargo test storage::

# Run with output
cargo test test_name -- --nocapture
```

### Code Quality

#### Automated Checks
Git hooks are **automatically configured** when entering the nix development environment:
- **pre-commit**: Runs `cargo fmt --check` to ensure code is formatted
- **pre-push**: Runs both `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings`

#### Manual Checks
```bash
# Run all quality checks
just final-check

# Individual checks
cargo clippy --locked --offline --workspace --all-targets --all-features -- --deny warnings
cargo fmt --all
cargo test
```

#### Managing Hooks
```bash
# Bypass hooks temporarily (not recommended)
git commit --no-verify
git push --no-verify

# Disable hooks completely
git config --unset core.hooksPath
```

## Environment Variables

| Variable | Description | Required | Default |
|----------|-------------|----------|---------|
| `TWITTER_BEARER_TOKEN` | Twitter API bearer token | Yes | - |
| `NOSTRWEET_DATA_DIR` | Data directory for all storage (tweets, media, profiles) | Yes (or use `-o` flag) | - |
| `NOSTRWEET_CACHE_DIR` | Additional cache directory path | No | - |
| `RUST_LOG` | Logging level | No | `info` |

## Contributing

1. Follow the coding conventions in `CONVENTIONS.md`
2. Add tests for new functionality
3. Run `just final-check` before submitting
4. Ensure all tests pass and code is properly formatted

## License

MIT License - see LICENSE file for details.
