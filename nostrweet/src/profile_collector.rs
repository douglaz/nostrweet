use crate::storage::find_latest_user_profile;
use crate::twitter::{ReferencedTweet, Tweet};
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use tracing::debug;

/// Collects all unique usernames referenced in a tweet
pub fn collect_usernames_from_tweet(tweet: &Tweet) -> HashSet<String> {
    let mut usernames = HashSet::new();

    // Add the main tweet author
    if !tweet.author.username.is_empty() {
        usernames.insert(tweet.author.username.clone());
    }

    // Add mentioned users
    if let Some(entities) = &tweet.entities
        && let Some(mentions) = &entities.mentions {
            for mention in mentions {
                usernames.insert(mention.username.clone());
            }
        }

    // Add authors from referenced tweets
    if let Some(ref_tweets) = &tweet.referenced_tweets {
        for ref_tweet in ref_tweets {
            collect_usernames_from_referenced_tweet(ref_tweet, &mut usernames);
        }
    }

    usernames
}

/// Collects usernames from a referenced tweet
fn collect_usernames_from_referenced_tweet(
    ref_tweet: &ReferencedTweet,
    usernames: &mut HashSet<String>,
) {
    if let Some(data) = &ref_tweet.data {
        // Add the referenced tweet's author
        if !data.author.username.is_empty() {
            usernames.insert(data.author.username.clone());
        }

        // Recursively collect from nested referenced tweets
        if let Some(nested_refs) = &data.referenced_tweets {
            for nested_ref in nested_refs {
                collect_usernames_from_referenced_tweet(nested_ref, usernames);
            }
        }

        // Add mentions from the referenced tweet
        if let Some(entities) = &data.entities
            && let Some(mentions) = &entities.mentions {
                for mention in mentions {
                    usernames.insert(mention.username.clone());
                }
            }
    }
}

/// Collects all unique usernames from multiple tweets
pub fn collect_usernames_from_tweets(tweets: &[Tweet]) -> HashSet<String> {
    let mut all_usernames = HashSet::new();

    for tweet in tweets {
        let tweet_usernames = collect_usernames_from_tweet(tweet);
        all_usernames.extend(tweet_usernames);
    }

    all_usernames
}

/// Filters out usernames that already have cached profiles
pub async fn filter_uncached_usernames(
    usernames: HashSet<String>,
    data_dir: &Path,
) -> Result<Vec<String>> {
    let mut uncached = Vec::new();

    for username in usernames {
        if find_latest_user_profile(&username, data_dir)?.is_none() {
            debug!("Profile for @{username} not found in cache, will download");
            uncached.push(username);
        } else {
            debug!("Profile for @{username} already cached, skipping");
        }
    }

    Ok(uncached)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::twitter::{Entities, Mention, User};

    fn create_test_user(username: &str) -> User {
        User {
            id: format!("{username}_id"),
            username: username.to_string(),
            name: Some(format!("{username} Name")),
            profile_image_url: None,
            description: None,
            url: None,
            entities: None,
        }
    }

    #[test]
    fn test_collect_usernames_from_simple_tweet() -> Result<()> {
        let tweet = Tweet {
            id: "123".to_string(),
            text: "Test tweet".to_string(),
            author: create_test_user("alice"),
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: None,
            includes: None,
            author_id: Some("alice_id".to_string()),
            note_tweet: None,
        };

        let usernames = collect_usernames_from_tweet(&tweet);
        assert_eq!(usernames.len(), 1);
        assert!(usernames.contains("alice"));

        Ok(())
    }

    #[test]
    fn test_collect_usernames_with_mentions() -> Result<()> {
        let tweet = Tweet {
            id: "123".to_string(),
            text: "Hello @bob and @charlie!".to_string(),
            author: create_test_user("alice"),
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: Some(Entities {
                urls: None,
                mentions: Some(vec![
                    Mention {
                        username: "bob".to_string(),
                    },
                    Mention {
                        username: "charlie".to_string(),
                    },
                ]),
                hashtags: None,
            }),
            includes: None,
            author_id: Some("alice_id".to_string()),
            note_tweet: None,
        };

        let usernames = collect_usernames_from_tweet(&tweet);
        assert_eq!(usernames.len(), 3);
        assert!(usernames.contains("alice"));
        assert!(usernames.contains("bob"));
        assert!(usernames.contains("charlie"));

        Ok(())
    }

    #[test]
    fn test_collect_usernames_with_referenced_tweets() -> Result<()> {
        let referenced_tweet = Tweet {
            id: "456".to_string(),
            text: "Original tweet by @david".to_string(),
            author: create_test_user("eve"),
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: Some(Entities {
                urls: None,
                mentions: Some(vec![Mention {
                    username: "david".to_string(),
                }]),
                hashtags: None,
            }),
            includes: None,
            author_id: Some("eve_id".to_string()),
            note_tweet: None,
        };

        let main_tweet = Tweet {
            id: "123".to_string(),
            text: "Replying to tweet".to_string(),
            author: create_test_user("alice"),
            referenced_tweets: Some(vec![ReferencedTweet {
                type_field: "replied_to".to_string(),
                id: "456".to_string(),
                data: Some(Box::new(referenced_tweet)),
            }]),
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: None,
            includes: None,
            author_id: Some("alice_id".to_string()),
            note_tweet: None,
        };

        let usernames = collect_usernames_from_tweet(&main_tweet);
        assert_eq!(usernames.len(), 3);
        assert!(usernames.contains("alice"));
        assert!(usernames.contains("eve"));
        assert!(usernames.contains("david"));

        Ok(())
    }

    #[test]
    fn test_collect_usernames_from_multiple_tweets() -> Result<()> {
        let tweet1 = Tweet {
            id: "1".to_string(),
            text: "Tweet 1".to_string(),
            author: create_test_user("alice"),
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: None,
            includes: None,
            author_id: Some("alice_id".to_string()),
            note_tweet: None,
        };

        let tweet2 = Tweet {
            id: "2".to_string(),
            text: "Tweet 2 mentioning @bob".to_string(),
            author: create_test_user("charlie"),
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: Some(Entities {
                urls: None,
                mentions: Some(vec![Mention {
                    username: "bob".to_string(),
                }]),
                hashtags: None,
            }),
            includes: None,
            author_id: Some("charlie_id".to_string()),
            note_tweet: None,
        };

        let tweets = vec![tweet1, tweet2];
        let usernames = collect_usernames_from_tweets(&tweets);

        assert_eq!(usernames.len(), 3);
        assert!(usernames.contains("alice"));
        assert!(usernames.contains("bob"));
        assert!(usernames.contains("charlie"));

        Ok(())
    }

    #[test]
    fn test_deduplication() -> Result<()> {
        // Create a tweet where the same user is mentioned multiple times
        let tweet = Tweet {
            id: "123".to_string(),
            text: "Hello @bob and @bob again!".to_string(),
            author: create_test_user("alice"),
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: Some(Entities {
                urls: None,
                mentions: Some(vec![
                    Mention {
                        username: "bob".to_string(),
                    },
                    Mention {
                        username: "bob".to_string(),
                    },
                ]),
                hashtags: None,
            }),
            includes: None,
            author_id: Some("alice_id".to_string()),
            note_tweet: None,
        };

        let usernames = collect_usernames_from_tweet(&tweet);
        // Should only have 2 unique usernames despite multiple mentions
        assert_eq!(usernames.len(), 2);
        assert!(usernames.contains("alice"));
        assert!(usernames.contains("bob"));

        Ok(())
    }
}
