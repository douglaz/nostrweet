#![forbid(unsafe_code)]

pub mod ids;
pub mod keys;
pub mod media;
pub mod nostr;
pub mod ports;
pub mod text;
pub mod twitter;

pub use ids::{
    BlossomUrl, CreatedAt, HttpUrl, MediaKey, NostrEventId, NostrPubkey, RelayUrl, TweetId,
    UnixTimestamp, UserId, Username,
};
pub use keys::{
    MnemonicPhrase, NostrSecretKey, account_index_for_user_id, derive_nostr_secret_key,
};
pub use media::{EnrichedTweet, Media, MediaKind, MediaVariant, RawTweet, extract_media_urls};
pub use nostr::{NostrEventDraft, NostrEventInfo, NostrEventResult, NostrTag};
pub use ports::{
    BlossomPort, Clock, MediaAsset, NostrPort, StoragePort, StoredTweet, StoredTweetSummary,
    TwitterPort, UserTweetsQuery,
};
pub use text::{ExpandedText, decode_html_entities, expand_urls_in_text};
pub use twitter::{
    Attachments, Entities, Hashtag, Includes, Mention, NoteTweet, ReferenceKind, ReferencedTweet,
    Tweet, TweetDraft, TweetText, UrlEntity, User, UserEntities, UserUrlEntity,
};
