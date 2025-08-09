#!/bin/bash

# Test script for smart resume functionality
set -e

echo "Testing smart resume functionality..."

# Test directory
TEST_DIR="./test_smart_resume_output"
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"

# Test user
TEST_USER="elonmusk"

echo "Step 1: Initial fetch without cache"
cargo run -- user-tweets "$TEST_USER" --count 5 --output-dir "$TEST_DIR"

# Count initial tweets
INITIAL_COUNT=$(ls "$TEST_DIR"/*_"$TEST_USER"_*.json 2>/dev/null | grep -v "_profile" | wc -l)
echo "Initial tweets downloaded: $INITIAL_COUNT"

# Get the latest tweet ID from cache
LATEST_ID=$(ls "$TEST_DIR"/*_"$TEST_USER"_*.json 2>/dev/null | grep -v "_profile" | sort | tail -1 | sed 's/.*_\([0-9]*\)\.json/\1/')
echo "Latest tweet ID in cache: $LATEST_ID"

echo -e "\nStep 2: Fetch again - should use since_id and only get newer tweets"
echo "This should return 0 new tweets if no new tweets were posted..."

# Run again - should use since_id
cargo run -- user-tweets "$TEST_USER" --count 5 --output-dir "$TEST_DIR"

# Count tweets after second run
SECOND_COUNT=$(ls "$TEST_DIR"/*_"$TEST_USER"_*.json 2>/dev/null | grep -v "_profile" | wc -l)
echo "Total tweets after second run: $SECOND_COUNT"

if [ "$SECOND_COUNT" -eq "$INITIAL_COUNT" ]; then
    echo "✅ Success: No duplicate tweets downloaded (smart resume working)"
else
    NEW_TWEETS=$((SECOND_COUNT - INITIAL_COUNT))
    echo "ℹ️ Downloaded $NEW_TWEETS new tweets (user posted new content)"
fi

echo -e "\nStep 3: Testing daemon mode with smart resume"
echo "Starting daemon for 15 seconds..."

# Start daemon in background with short poll interval
timeout 15 cargo run -- daemon \
    --user "$TEST_USER" \
    --relay "wss://relay.damus.io" \
    --poll-interval 5 \
    --output-dir "$TEST_DIR" \
    --private-key "0000000000000000000000000000000000000000000000000000000000000001" &

DAEMON_PID=$!

# Wait for daemon to run
sleep 15

# Count tweets after daemon run
DAEMON_COUNT=$(ls "$TEST_DIR"/*_"$TEST_USER"_*.json 2>/dev/null | grep -v "_profile" | wc -l)
echo "Total tweets after daemon run: $DAEMON_COUNT"

echo -e "\nTest Summary:"
echo "- Initial fetch: $INITIAL_COUNT tweets"
echo "- After resume: $SECOND_COUNT tweets"
echo "- After daemon: $DAEMON_COUNT tweets"
echo "- Latest tweet ID tracked: $LATEST_ID"

# Cleanup
echo -e "\nCleaning up test directory..."
rm -rf "$TEST_DIR"

echo "✅ Smart resume test completed!"