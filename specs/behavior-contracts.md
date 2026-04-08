# Behavior Contracts (Legacy Implementation)

Source of truth: `crates/nostrweet-cli/src/main.rs`, `crates/nostrweet-storage/src/lib.rs`, `crates/nostrweet-core/src/{ids.rs,keys.rs,media.rs,nostr.rs,text.rs,twitter.rs}`, `crates/nostrweet-twitter/src/lib.rs`, `crates/nostrweet-blossom/src/lib.rs`.

Do NOT treat README as spec. This document encodes observed behavior from code.

## CLI Contract

Binary name: `nostrweet`

### Global Flags

- `-o, --data-dir <DIR>` (env: `NOSTRWEET_DATA_DIR`) optional in CLI but REQUIRED at runtime; fallback env `NOSTRWEET_OUTPUT_DIR` (legacy).
  - If not provided, the CLI errors with: "Data directory not specified. Please set --data-dir or NOSTRWEET_DATA_DIR environment variable".
  - If path does not exist, it is created.
- `--bearer-token <TOKEN>` (env: `TWITTER_BEARER_TOKEN`) optional globally.
  - REQUIRED for commands that contact Twitter API: `fetch-profile`, `fetch-tweet`, `user-tweets`, `daemon`.
- `-m, --mnemonic <MNEMONIC>` (env: `NOSTRWEET_MNEMONIC`) optional globally.
- `-v, --verbose` enables verbose logging.

### Commands and Options

- `fetch-profile <username>`
  - Required arg: twitter username (with or without `@`).
- `fetch-tweet <tweet_url_or_id>`
  - Required arg: tweet ID or URL.
  - `--skip-profiles` (bool, default false) skips profile downloads for referenced users.
- `user-tweets <username>`
  - Required arg: twitter username.
  - `-c, --count <N>` default `10`.
  - `--days <N>` (optional) filter by recent days.
  - `--skip-profiles` (bool, default false).
- `list-tweets`
- `clear-cache`
  - `-f, --force` skip confirmation prompt.
- `post-tweet-to-nostr <tweet_url_or_id>`
  - Required arg: tweet ID or URL.
  - `-r, --relays <RELAYS>` (comma-separated, env `NOSTRWEET_RELAYS`) REQUIRED.
  - `--blossom-servers <URLS>` (comma-separated, env `NOSTRWEET_BLOSSOM_SERVERS`) optional.
  - `-f, --force` overwrite existing event.
  - `--skip-profiles` (bool, default false).
- `post-user-to-nostr <username>`
  - Required arg: twitter username.
  - `-r, --relays <RELAYS>` (comma-separated, env `NOSTRWEET_RELAYS`) REQUIRED.
  - `--blossom-servers <URLS>` (comma-separated, env `NOSTRWEET_BLOSSOM_SERVERS`) optional.
  - `-f, --force` overwrite existing events.
  - `--skip-profiles` (bool, default false).
- `post-tweet <tweet_url_or_id>`
  - Same flags as `post-tweet-to-nostr`.
- `post-profile-to-nostr <username>`
  - Required arg: twitter username.
  - `-r, --relay <RELAYS>` (comma-separated, env `NOSTRWEET_RELAYS`) REQUIRED.
- `update-relay-list`
  - `-r, --relays <RELAYS>` (comma-separated, env `NOSTRWEET_RELAYS`) REQUIRED.
- `show-tweet <tweet_id_or_url>`
  - `--pretty` (bool, default false, but output is pretty unless `--compact` is set).
  - `--compact` (bool) outputs single-line JSON.
- `daemon`
  - `--user <username>` (repeatable, REQUIRED).
  - `--relay <relay>` (repeatable, REQUIRED).
  - `--blossom-server <url>` (repeatable).
  - `--poll-interval <seconds>` default `300`.
- `utils query-events`
  - `--relay <relay>` (repeatable, REQUIRED).
  - `-k, --kind <kind>` optional.
  - `-a, --author <hex_or_npub>` optional.
  - `-l, --limit <N>` default `10`.
  - `--since <unix>` optional.
  - `--until <unix>` optional.
  - `-f, --format <json|pretty>` default `pretty`.
  - `--output <path>` optional.

### Exit Behavior

- Commands return errors via `anyhow` with contextual messages (see source).
- `clear-cache` prompts unless `--force` is set; anything other than `y`/`Y` cancels.

## Storage & File Layout Contract

All files live directly under `data_dir` unless noted.

### Tweet JSON

Filename: `YYYYMMDD_HHMMSS_<username>_<tweet_id>.json`
- Timestamp comes from tweet `created_at` (RFC3339), not current time.
- Filename is sanitized (no `/` or `\`).

### User Profile JSON

Filename: `YYYYMMDDHHMMSS_<username>_<user_id>.json`
- Timestamp is **current time** (compact format, no underscore in date/time).

### Media Files

Filename: `<username>_<media_key_clean>.<ext>`
- `media_key_clean` strips prefix before `_` (e.g. `3_123` -> `123`).
- Extensions: `jpg` for photos, `mp4` for video/animated_gif.

### Nostr Event JSON

Directory: `data_dir/nostr_events/`
- Filename: `<event_id_hex>.json`

### Nostr Event Info

Directory: `data_dir/nostr/`
- Filename: `<tweet_id>.json`
- JSON shape (`nostr::NostrEventInfo`):
  - `tweet_id`, `event_id`, `pubkey`, `created_at`, `media_urls`, `relays`, `event_json` (optional string).

### Not-Found Marker

Filename: `<tweet_id>.not_found` (empty file). Used to mark missing tweets.

### Cache-as-State (Daemon)

- Latest tweet ID per user inferred from filenames matching `*_<username>_*.json` (excluding `_profile`).
- `.not_found` is a sentinel for missing tweets.

## Tweet -> Nostr Formatting Contract

Formatting functions live in `crates/nostrweet-cli/src/main.rs`.

### Common Rules

- Base URL for original tweet: `https://twitter.com/i/status/<tweet_id>`.
- If `note_tweet` is present, use `note_tweet.text` (full text) instead of `text`.
- HTML entities are decoded before expansion (e.g., `&gt;` -> `>`).
- URL expansion:
  - Uses `entities.urls`.
  - Non-media URLs are replaced with markdown: `[display_url](expanded_url)`.
  - Media URLs (t.co -> media) are replaced inline with direct media URLs.
  - When media URLs are used inline, they should NOT be duplicated in the media list later.
- Mentions:
  - `format_tweet_as_nostr_content_with_mentions` resolves `@username` to `nostr:npub...` when possible (using cached profiles + mnemonic-derived keys).
  - Unresolvable users remain as `@username`.

### Standard Tweet

Prefix:
- `🐦 @<username>: ` if username is available.
- `🐦 User <id>: ` if username missing but `author.id` or `author_id` exists.
- `🐦 Tweet: ` if no author info.

Content:
- Expanded tweet text.
- Any media URLs not already inlined are appended each on a new line.
- Blank line then `Original tweet: https://twitter.com/i/status/<id>`.

### Reply

- Main tweet formatted as standard tweet content (with mentions and URL expansion).
- Then:
  - `↩️ Reply to @<username>:` or `↩️ Reply to nostr:npub...:` when resolved.
  - Full referenced tweet text with URL/media expansion.
  - Referenced tweet URL (`https://twitter.com/i/status/<ref_id>`).
- Ends with original tweet link.

### Retweet

- Header:
  - Simple retweet: `🔁 @<retweeter> retweeted @<original>:`.
  - With mention resolution: `🔁 @<retweeter> retweeted nostr:npub...:` when resolved.
- Body is the referenced tweet content with URL expansion.
- Media URLs from referenced tweet should be inlined where the t.co URL appeared.
- Includes referenced tweet URL and original tweet link.

### Quote Tweet

- Header:
  - `💬 Quote of @<username>:` or `💬 Quote of nostr:npub...:` when resolved.
- Includes quoted tweet content, then quote tweet URL.
- Ends with original tweet link.

## Nostr Tags Contract (post-tweet-to-nostr)

- Always includes `r` tag with original tweet URL.
- Adds `p` tags for mentioned pubkeys.
- Media tags:
  - If no Blossom upload: `media` tags contain original media URLs.
  - If Blossom upload: `source` tags for original URLs and `media` tags for Blossom URLs (paired order).
- Includes `client` tag with value `nostrweet`.

## show-tweet Output Contract

`show-tweet` prints JSON **to stdout** containing:

```json
{
  "twitter": <tweet object>,
  "nostr": {
    "event": <nostr event>,
    "metadata": {
      "original_tweet_id": "...",
      "original_author": "...",
      "created_at_human": "...",
      "content_preview": "...",
      "tags_count": <number>,
      "pubkey_hex": "...",
      "event_id_hex": "..."
    }
  }
}
```

- Default output is pretty JSON unless `--compact` is set.
- `--pretty` does not override `--compact`.
- Logs go to stderr; stdout must remain pure JSON (tests enforce no log lines in stdout).

## list-tweets Output Contract

- Reads all `.json` files in `data_dir` (tweet files and other JSON files) and tries to parse them as tweets; non-tweet JSON files are skipped with a warning.
- Sorts by file modification time (newest first).
- Output format:
  - `Found <N> tweets in <data_dir>`
  - Separator line of 80 chars
  - For each tweet:
    - `ID: <tweet_id>`
    - `  │ Author: <name (@username)>` or `@username` or `ID: <author_id>` or `Unknown`
    - `Text: <first_line_of_text>`
    - `Date: <modified_time_formatted>`
    - `File: <filename>`
    - Separator line

## Tweet ID Parsing Contract

`parse_tweet_id` accepts:
- Pure numeric IDs.
- URLs matching `https://twitter.com/<user>/status/<id>` or `https://x.com/...` (including query params).
- Regex fallback: `(?:twitter\.com|x\.com)/\w+/status/(\d+)`.
- Errors on empty/invalid input.

## Key Derivation Contract

- Nostr keys are deterministically derived from Twitter user ID + mnemonic.
- Mnemonic is REQUIRED when posting to Nostr; absence results in an error.
- Derivation is deterministic for a given user ID + mnemonic.

## Notes on Existing Tests

- Integration tests in `crates/nostrweet-integration-tests/` cover end-to-end behavior with real network calls.
