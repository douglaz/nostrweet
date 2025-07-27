#!/usr/bin/env bash
set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Default values
RELAY_PORT=7000
CLEANUP=true
VERBOSE=false

# Get script directory
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Load .env file if it exists
if [ -f "$PROJECT_ROOT/.env" ]; then
    set -a
    source "$PROJECT_ROOT/.env"
    set +a
fi

# Logging functions (must be defined before usage)
log() {
    echo -e "${GREEN}[$(date +'%Y-%m-%d %H:%M:%S')]${NC} $1"
}

error() {
    echo -e "${RED}[$(date +'%Y-%m-%d %H:%M:%S')] ERROR:${NC} $1" >&2
}

warn() {
    echo -e "${YELLOW}[$(date +'%Y-%m-%d %H:%M:%S')] WARN:${NC} $1"
}

# Check for Twitter token
if [ -z "${TWITTER_BEARER_TOKEN:-}" ]; then
    error "TWITTER_BEARER_TOKEN not found in environment or .env file"
    echo "Please set TWITTER_BEARER_TOKEN in your .env file or environment"
    exit 1
fi

# Usage
usage() {
    echo "Usage: $0 [options]"
    echo "Options:"
    echo "  --no-cleanup    Don't clean up Docker container after test"
    echo "  --verbose       Enable verbose output"
    echo "  --port PORT     Port for Nostr relay (default: 7000)"
    echo ""
    echo "Twitter Bearer Token is read from .env file or TWITTER_BEARER_TOKEN environment variable"
    exit 1
}

# Parse arguments

while [[ $# -gt 0 ]]; do
    case $1 in
        --no-cleanup)
            CLEANUP=false
            shift
            ;;
        --verbose)
            VERBOSE=true
            shift
            ;;
        --port)
            RELAY_PORT="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            usage
            ;;
    esac
done

# Cleanup function
cleanup() {
    if [ "$CLEANUP" = true ]; then
        log "Cleaning up..."
        docker stop nostr-relay-test 2>/dev/null || true
        docker rm nostr-relay-test 2>/dev/null || true
        rm -rf "$TEST_DIR" 2>/dev/null || true
    else
        log "Skipping cleanup (--no-cleanup flag set)"
        log "Test directory: $TEST_DIR"
        log "Docker container: nostr-relay-test"
    fi
}

# Set up trap for cleanup
trap cleanup EXIT

# Create temporary directory
TEST_DIR=$(mktemp -d -t nostrweet-e2e-XXXXXX)
log "Created test directory: $TEST_DIR"

# Check prerequisites
log "Checking prerequisites..."

if ! command -v docker &> /dev/null; then
    error "Docker is required but not installed"
    exit 1
fi

if ! command -v cargo &> /dev/null; then
    error "Cargo is required but not installed"
    exit 1
fi

if ! command -v websocat &> /dev/null; then
    warn "websocat not found, installing..."
    cargo install websocat || {
        error "Failed to install websocat"
        exit 1
    }
fi

# Start Nostr relay
log "Starting Nostr relay on port $RELAY_PORT..."
docker run -d \
    --name nostr-relay-test \
    -p "$RELAY_PORT:8080" \
    -e RUST_LOG=info \
    scsibug/nostr-rs-relay:latest || {
    error "Failed to start Nostr relay"
    exit 1
}

# Wait for relay to be ready
log "Waiting for relay to be ready..."
for i in {1..30}; do
    if curl -s "http://localhost:$RELAY_PORT" >/dev/null 2>&1; then
        log "Relay is ready!"
        break
    fi
    if [ $i -eq 30 ]; then
        error "Relay failed to start after 30 seconds"
        exit 1
    fi
    sleep 1
done

# Generate test keys
log "Generating test Nostr keys..."
PRIVATE_KEY=$(openssl rand -hex 32)
log "Private key: $PRIVATE_KEY"

# Build the project
log "Building nostrweet..."
cargo build --release || {
    error "Failed to build project"
    exit 1
}

# Test tweet ID (a popular tweet that should always exist)
# Using Twitter's first tweet
TWEET_ID="20"

# Fetch and post tweet
log "Fetching tweet $TWEET_ID and posting to Nostr..."
OUTPUT_DIR="$TEST_DIR/downloads"

./target/release/nostrweet fetch-tweet \
    --output-dir "$OUTPUT_DIR" \
    "$TWEET_ID" || {
    error "Failed to fetch tweet"
    exit 1
}

# Find the downloaded tweet file
TWEET_FILE=$(find "$OUTPUT_DIR" -name "*_${TWEET_ID}.json" -type f | head -1)
if [ -z "$TWEET_FILE" ]; then
    error "Tweet file not found in $OUTPUT_DIR"
    exit 1
fi

log "Tweet downloaded to: $TWEET_FILE"

# Post to Nostr
log "Posting tweet to Nostr relay..."
./target/release/nostrweet post-tweet-to-nostr \
    --private-key "$PRIVATE_KEY" \
    --relays "ws://localhost:$RELAY_PORT" \
    --output-dir "$OUTPUT_DIR" \
    "$TWEET_ID" || {
    error "Failed to post tweet to Nostr"
    exit 1
}

# Wait a moment for the event to propagate
sleep 2

# Verify the event was posted
log "Verifying event on Nostr relay..."

# Get public key from private key using Python (simpler and more reliable)
PUBLIC_KEY=$(python3 -c "
import hashlib
import binascii

# secp256k1 parameters
p = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F
n = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
Gx = 0x79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798
Gy = 0x483ADA7726A3C4655DA4FBFC0E1108A8FD17B448A68554199C47D08FFB10D4B8

def modinv(a, m):
    return pow(a, m - 2, m)

def point_add(p1, p2):
    if p1 is None:
        return p2
    if p2 is None:
        return p1
    x1, y1 = p1
    x2, y2 = p2
    if x1 == x2:
        if y1 == y2:
            s = (3 * x1 * x1 * modinv(2 * y1, p)) % p
        else:
            return None
    else:
        s = ((y2 - y1) * modinv(x2 - x1, p)) % p
    x3 = (s * s - x1 - x2) % p
    y3 = (s * (x1 - x3) - y1) % p
    return (x3, y3)

def scalar_mult(k, point):
    if k == 0:
        return None
    result = None
    addend = point
    while k:
        if k & 1:
            result = point_add(result, addend)
        addend = point_add(addend, addend)
        k >>= 1
    return result

private_key = int('$PRIVATE_KEY', 16)
public_key_point = scalar_mult(private_key, (Gx, Gy))
x = public_key_point[0]
# Compressed format: 02 or 03 prefix based on y coordinate parity
y = public_key_point[1]
prefix = '02' if y % 2 == 0 else '03'
public_key = prefix + format(x, '064x')
# For Nostr, we use the x-coordinate only (32 bytes)
print(format(x, '064x'))
" 2>/dev/null)

if [ -z "$PUBLIC_KEY" ]; then
    error "Failed to derive public key from private key"
    exit 1
fi

log "Public key: $PUBLIC_KEY"

# Create a subscription request
SUBSCRIPTION_ID=$(openssl rand -hex 8)
REQUEST='["REQ","'$SUBSCRIPTION_ID'",{"authors":["'$PUBLIC_KEY'"]}]'

if [ "$VERBOSE" = true ]; then
    log "Subscription request: $REQUEST"
fi

# Query the relay
RESPONSE=$(echo "$REQUEST" | websocat -t -n1 "ws://localhost:$RELAY_PORT" 2>/dev/null | grep -E '^\["EVENT"' | head -1)

if [ -z "$RESPONSE" ]; then
    error "No events found on relay"
    exit 1
fi

if [ "$VERBOSE" = true ]; then
    log "Relay response: $RESPONSE"
fi

# Parse response to verify content
if echo "$RESPONSE" | grep -q "just setting up my twttr"; then
    log "✅ Successfully verified tweet content on Nostr relay!"
else
    error "Tweet content not found in Nostr event"
    exit 1
fi

# Additional verification: check event structure
if echo "$RESPONSE" | grep -q '"kind":1'; then
    log "✅ Event has correct kind (1)"
else
    error "Event has incorrect kind"
    exit 1
fi

# Test with media tweet (optional)
log "Testing tweet with media..."
MEDIA_TWEET_ID="1580661436132073472"  # Example tweet with image

./target/release/nostrweet fetch-tweet \
    --output-dir "$OUTPUT_DIR" \
    "$MEDIA_TWEET_ID" || {
    warn "Failed to fetch media tweet (might be deleted/protected)"
}

# Find the media tweet file
MEDIA_TWEET_FILE=$(find "$OUTPUT_DIR" -name "*_${MEDIA_TWEET_ID}.json" -type f | head -1)
if [ -n "$MEDIA_TWEET_FILE" ]; then
    log "Posting media tweet to Nostr..."
    ./target/release/nostrweet post-tweet-to-nostr \
        --private-key "$PRIVATE_KEY" \
        --relays "ws://localhost:$RELAY_PORT" \
        --output-dir "$OUTPUT_DIR" \
        "$MEDIA_TWEET_ID" || {
        warn "Failed to post media tweet to Nostr"
    }
fi

log "✅ End-to-end test completed successfully!"
log "Summary:"
log "  - Started temporary Nostr relay on port $RELAY_PORT"
log "  - Fetched real tweet from Twitter"
log "  - Posted tweet to Nostr relay"
log "  - Verified event was correctly stored and retrievable"

if [ "$CLEANUP" = false ]; then
    log ""
    log "Resources kept for inspection:"
    log "  - Docker container: nostr-relay-test"
    log "  - Test directory: $TEST_DIR"
    log ""
    log "To manually clean up:"
    log "  docker stop nostr-relay-test && docker rm nostr-relay-test"
    log "  rm -rf $TEST_DIR"
fi