#![forbid(unsafe_code)]

use anyhow::{Context, Result, anyhow};
use nostrweet_core::{
    NostrEventDraft, NostrEventId, NostrEventResult, NostrPort, RelayUrl, TweetId,
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct InMemoryNostr {
    state: Arc<Mutex<State>>,
}

#[derive(Debug)]
struct State {
    next_id: u64,
    events: Vec<StoredEvent>,
    relays: Vec<RelayUrl>,
}

#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub event_id: NostrEventId,
    pub draft: NostrEventDraft,
    pub event_json: Option<String>,
}

impl InMemoryNostr {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                next_id: 1,
                events: Vec::new(),
                relays: Vec::new(),
            })),
        }
    }

    pub fn relay_list(&self) -> Result<Vec<RelayUrl>> {
        let state = self.lock_state()?;
        Ok(state.relays.clone())
    }

    pub fn published_events(&self) -> Result<Vec<StoredEvent>> {
        let state = self.lock_state()?;
        Ok(state.events.clone())
    }

    fn twitter_status_url(tweet_id: &TweetId) -> String {
        format!("https://twitter.com/i/status/{}", tweet_id.as_str())
    }

    fn event_references_tweet(event: &StoredEvent, tweet_id: &TweetId) -> bool {
        let target = Self::twitter_status_url(tweet_id);
        event.draft.tags.iter().any(|tag| {
            tag.name.eq_ignore_ascii_case("r") && tag.values.iter().any(|value| value == &target)
        })
    }

    fn next_event_id(state: &mut State) -> Result<NostrEventId> {
        let id_hex = format!("{:064x}", state.next_id);
        state.next_id = state.next_id.saturating_add(1);
        NostrEventId::parse(&id_hex).context("Failed to build event id")
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("Nostr adapter mutex poisoned"))
    }
}

impl Default for InMemoryNostr {
    fn default() -> Self {
        Self::new()
    }
}

impl NostrPort for InMemoryNostr {
    async fn publish_event(&self, event: NostrEventDraft) -> Result<NostrEventResult> {
        let mut state = self.lock_state()?;
        let event_id = Self::next_event_id(&mut state)?;
        let result = NostrEventResult {
            event_id: event_id.clone(),
            event_json: None,
        };

        state.events.push(StoredEvent {
            event_id,
            draft: event,
            event_json: result.event_json.clone(),
        });

        Ok(result)
    }

    async fn update_relay_list(&self, relays: &[RelayUrl]) -> Result<()> {
        let mut state = self.lock_state()?;
        state.relays = relays.to_vec();
        Ok(())
    }

    async fn find_event_by_tweet(&self, tweet_id: &TweetId) -> Result<Option<NostrEventResult>> {
        let state = self.lock_state()?;
        for event in state.events.iter().rev() {
            if Self::event_references_tweet(event, tweet_id) {
                return Ok(Some(NostrEventResult {
                    event_id: event.event_id.clone(),
                    event_json: event.event_json.clone(),
                }));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostrweet_core::{HttpUrl, NostrTag};

    fn event_with_tweet_ref(tweet_id: &TweetId) -> Result<NostrEventDraft> {
        let url = HttpUrl::parse(&InMemoryNostr::twitter_status_url(tweet_id))?;
        Ok(NostrEventDraft {
            content: "hello".to_string(),
            tags: vec![NostrTag::r(&url)],
            created_at: None,
        })
    }

    #[tokio::test]
    async fn publish_event_assigns_incrementing_ids() -> Result<()> {
        let adapter = InMemoryNostr::new();

        let first = adapter
            .publish_event(NostrEventDraft {
                content: "one".to_string(),
                tags: Vec::new(),
                created_at: None,
            })
            .await?;
        let second = adapter
            .publish_event(NostrEventDraft {
                content: "two".to_string(),
                tags: Vec::new(),
                created_at: None,
            })
            .await?;

        assert_ne!(first.event_id, second.event_id);
        assert_eq!(first.event_id.as_str(), &format!("{:064x}", 1));
        assert_eq!(second.event_id.as_str(), &format!("{:064x}", 2));
        Ok(())
    }

    #[tokio::test]
    async fn find_event_by_tweet_matches_r_tag() -> Result<()> {
        let adapter = InMemoryNostr::new();
        let tweet_id = TweetId::parse("123")?;
        let other_id = TweetId::parse("999")?;

        let event = event_with_tweet_ref(&tweet_id)?;
        let published = adapter.publish_event(event).await?;

        let found = adapter.find_event_by_tweet(&tweet_id).await?;
        assert_eq!(
            found.map(|result| result.event_id),
            Some(published.event_id)
        );

        let missing = adapter.find_event_by_tweet(&other_id).await?;
        assert!(missing.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn update_relay_list_overwrites_state() -> Result<()> {
        let adapter = InMemoryNostr::new();
        let relays = vec![
            RelayUrl::parse("https://relay1.example")?,
            RelayUrl::parse("https://relay2.example")?,
        ];

        adapter.update_relay_list(&relays).await?;
        let stored = adapter.relay_list()?;
        assert_eq!(stored, relays);

        let relays = vec![RelayUrl::parse("https://relay3.example")?];
        adapter.update_relay_list(&relays).await?;
        let stored = adapter.relay_list()?;
        assert_eq!(stored, relays);
        Ok(())
    }
}
