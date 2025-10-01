use nostr_sdk::{EventBuilder, Keys, Kind, Tag, Timestamp};
use nostrweet::media::extract_media_urls_from_tweet;
use nostrweet::twitter::Tweet;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

/// Helper function for legacy test compatibility (simplified formatting without mentions)
fn format_tweet_as_nostr_content(tweet: &Tweet, media_urls: &[String]) -> String {
    // This is a simplified legacy implementation for tests only
    // Real applications should use format_tweet_as_nostr_content_with_mentions
    let mut content = String::new();

    // Extract media URLs from tweet if none provided
    let actual_media_urls = if media_urls.is_empty() {
        extract_media_urls_from_tweet(tweet)
    } else {
        media_urls.to_vec()
    };

    // Check if this is a retweet
    if let Some(ref_tweets) = &tweet.referenced_tweets
        && let Some(retweet) = ref_tweets.iter().find(|rt| rt.type_field == "retweeted")
        && let Some(rt_data) = &retweet.data
    {
        content.push_str(&format!(
            "🔁 @{} retweeted @{}:",
            tweet.author.username, rt_data.author.username
        ));
        content.push('\n');

        // Expand URLs in retweeted content if entities exist
        let mut expanded_rt_text = rt_data.text.clone();
        if let Some(entities) = &rt_data.entities
            && let Some(urls) = &entities.urls
        {
            for url_entity in urls {
                if let Some(expanded) = &url_entity.expanded_url {
                    // Check if this is a media URL and use actual media URL if available
                    let actual_url = if expanded.contains("/video/")
                        || expanded.contains("pic.x.com")
                        || expanded.contains("pic.twitter.com")
                        || expanded.contains("/photo/")
                        || (expanded.contains("/status/")
                            && (expanded.contains("twitter.com") || expanded.contains("x.com")))
                    {
                        // For media URLs, use the first available extracted media URL
                        if !actual_media_urls.is_empty() {
                            actual_media_urls[0].clone()
                        } else {
                            expanded.clone()
                        }
                    } else {
                        expanded.clone()
                    };

                    expanded_rt_text = expanded_rt_text.replace(&url_entity.url, &actual_url);
                }
            }
        }

        content.push_str(&expanded_rt_text);
        content.push('\n');
        content.push_str(&format!("https://twitter.com/i/status/{}", retweet.id));
        content.push_str(&format!(
            "\n\nOriginal tweet: https://twitter.com/i/status/{}",
            tweet.id
        ));
        return content;
    }

    // Check if this is a reply
    if let Some(ref_tweets) = &tweet.referenced_tweets
        && let Some(reply) = ref_tweets.iter().find(|rt| rt.type_field == "replied_to")
        && let Some(reply_data) = &reply.data
    {
        content.push_str(&format!("🐦 @{}: ", tweet.author.username));

        // Expand URLs in the reply tweet
        let mut expanded_reply_text = tweet.text.clone();
        if let Some(entities) = &tweet.entities
            && let Some(urls) = &entities.urls
        {
            for url_entity in urls {
                if let Some(expanded) = &url_entity.expanded_url {
                    // Check for media URLs and use actual media URL if available
                    let actual_url = if expanded.contains("/video/")
                        || expanded.contains("pic.x.com")
                        || expanded.contains("pic.twitter.com")
                    {
                        // Try to find a matching media URL from the extracted ones
                        let mut found_media_url = None;
                        for media_url in &actual_media_urls {
                            if media_url.contains("twimg.com") || media_url.contains("video") {
                                found_media_url = Some(media_url.clone());
                                break;
                            }
                        }
                        found_media_url.unwrap_or_else(|| expanded.clone())
                    } else {
                        expanded.clone()
                    };

                    // For media URLs, replace inline; for others use markdown
                    if actual_url.contains("twimg.com") || actual_url.contains("video") {
                        expanded_reply_text =
                            expanded_reply_text.replace(&url_entity.url, &actual_url);
                    } else {
                        expanded_reply_text = expanded_reply_text.replace(
                            &url_entity.url,
                            &format!("[{}]({})", url_entity.display_url, actual_url),
                        );
                    }
                }
            }
        }

        content.push_str(&expanded_reply_text);
        content.push_str(&format!("\n\n↩️ Reply to @{}:", reply_data.author.username));
        content.push('\n');

        // Expand URLs in the referenced tweet
        let mut expanded_ref_text = reply_data.text.clone();
        if let Some(entities) = &reply_data.entities
            && let Some(urls) = &entities.urls
        {
            for url_entity in urls {
                if let Some(expanded) = &url_entity.expanded_url {
                    // Check for media URLs and use actual media URL if available
                    let actual_url = if expanded.contains("/video/")
                        || expanded.contains("pic.x.com")
                        || expanded.contains("pic.twitter.com")
                        || expanded.contains("/photo/")
                        || (expanded.contains("/status/")
                            && (expanded.contains("twitter.com") || expanded.contains("x.com")))
                    {
                        // For media URLs, use the first available extracted media URL
                        if !actual_media_urls.is_empty() {
                            actual_media_urls[0].clone()
                        } else {
                            expanded.clone()
                        }
                    } else {
                        expanded.clone()
                    };

                    // For media URLs, replace inline; for others use markdown
                    if actual_url.contains("twimg.com")
                        || actual_url.contains("video")
                        || actual_url.contains("pbs.twimg.com")
                    {
                        expanded_ref_text = expanded_ref_text.replace(&url_entity.url, &actual_url);
                    } else {
                        expanded_ref_text = expanded_ref_text.replace(
                            &url_entity.url,
                            &format!("[{}]({})", url_entity.display_url, actual_url),
                        );
                    }
                }
            }
        }

        content.push_str(&expanded_ref_text);
        content.push_str(&format!("\nhttps://twitter.com/i/status/{}", reply.id));
        content.push_str(&format!(
            "\n\nOriginal tweet: https://twitter.com/i/status/{}",
            tweet.id
        ));
        return content;
    }

    // Check if this is a quote tweet
    if let Some(ref_tweets) = &tweet.referenced_tweets
        && let Some(quote) = ref_tweets.iter().find(|rt| rt.type_field == "quoted")
        && let Some(quote_data) = &quote.data
    {
        content.push_str(&format!("🐦 @{}: ", tweet.author.username));
        content.push_str(&tweet.text);
        content.push_str(&format!("\n\n💬 Quote of @{}:", quote_data.author.username));
        content.push('\n');
        content.push_str(&quote_data.text);
        content.push_str(&format!("\nhttps://twitter.com/i/status/{}", quote.id));
        content.push_str(&format!(
            "\n\nOriginal tweet: https://twitter.com/i/status/{}",
            tweet.id
        ));
        return content;
    }

    // Check for note tweet (long form content)
    let tweet_text = if let Some(ref note) = tweet.note_tweet {
        &note.text
    } else {
        &tweet.text
    };

    // Handle missing author info
    let username = if tweet.author.username.is_empty() {
        // Try author.id first, then author_id as fallback
        if !tweet.author.id.is_empty() {
            format!("User {}", tweet.author.id)
        } else if let Some(author_id) = &tweet.author_id {
            if !author_id.is_empty() {
                format!("User {author_id}")
            } else {
                "Tweet".to_string()
            }
        } else {
            "Tweet".to_string()
        }
    } else {
        tweet.author.username.clone()
    };

    // Regular tweet - expand URLs if entities exist
    let mut expanded_text = tweet_text.clone();
    if let Some(entities) = &tweet.entities
        && let Some(urls) = &entities.urls
    {
        for url_entity in urls {
            if let Some(expanded) = &url_entity.expanded_url {
                // Check if this is a media URL and use actual media URL if available
                let actual_url = if expanded.contains("/video/")
                    || expanded.contains("pic.x.com")
                    || expanded.contains("pic.twitter.com")
                {
                    // First try to find a matching media URL from the extracted ones
                    let mut found_media_url = None;
                    for media_url in &actual_media_urls {
                        if media_url.contains("twimg.com") || media_url.contains("video") {
                            found_media_url = Some(media_url.clone());
                            break;
                        }
                    }

                    // Use the found media URL or fall back to expanded URL
                    found_media_url.unwrap_or_else(|| expanded.clone())
                } else {
                    expanded.clone()
                };

                // For media URLs, just replace with the actual URL inline
                if actual_url.contains("twimg.com") || actual_url.contains("video") {
                    expanded_text = expanded_text.replace(&url_entity.url, &actual_url);
                } else {
                    // For non-media URLs, use markdown format
                    expanded_text = expanded_text.replace(
                        &url_entity.url,
                        &format!("[{}]({})", url_entity.display_url, actual_url),
                    );
                }
            }
        }
    }

    // Basic tweet format
    if username.starts_with("User ") || username == "Tweet" {
        content.push_str(&format!("🐦 {username}: "));
    } else {
        content.push_str(&format!("🐦 @{username}: "));
    }
    content.push_str(&expanded_text);
    content.push_str("\n\n");

    // Add media URLs
    for url in media_urls {
        content.push_str(&format!("{url}\n"));
    }

    // Add original URL
    content.push_str(&format!(
        "\nOriginal tweet: https://twitter.com/i/status/{}",
        tweet.id
    ));

    content
}

/// Test data based on real tweet structures to ensure consistent parsing
mod fixtures {
    use super::*;

    /// A simple tweet with text only
    pub fn simple_tweet() -> Tweet {
        serde_json::from_value(json!({
            "id": "1234567890123456789",
            "text": "Hello Twitter! This is a test tweet.",
            "created_at": "2023-01-15T10:30:00.000Z",
            "author_id": "987654321",
            "author": {
                "id": "987654321",
                "username": "testuser",
                "name": "Test User"
            }
        }))
        .unwrap()
    }

    /// A tweet with a URL that should be expanded
    pub fn tweet_with_url() -> Tweet {
        serde_json::from_value(json!({
            "id": "1234567890123456790",
            "text": "Check out this article: https://t.co/abc123def",
            "created_at": "2023-01-15T11:00:00.000Z",
            "author_id": "987654321",
            "author": {
                "id": "987654321",
                "username": "testuser",
                "name": "Test User"
            },
            "entities": {
                "urls": [{
                    "url": "https://t.co/abc123def",
                    "expanded_url": "https://example.com/interesting-article",
                    "display_url": "example.com/interesting-ar..."
                }]
            }
        }))
        .unwrap()
    }

    /// A retweet
    pub fn retweet() -> Tweet {
        serde_json::from_value(json!({
            "id": "1234567890123456791",
            "text": "RT @originaluser: This is the original tweet content that was retweeted",
            "created_at": "2023-01-15T12:00:00.000Z",
            "author_id": "987654321",
            "author": {
                "id": "987654321",
                "username": "retweeter",
                "name": "Retweeting User"
            },
            "referenced_tweets": [{
                "type": "retweeted",
                "id": "1234567890123456700",
                "data": {
                    "id": "1234567890123456700",
                    "text": "This is the original tweet content that was retweeted",
                    "created_at": "2023-01-15T09:00:00.000Z",
                    "author_id": "111111111",
                    "author": {
                        "id": "111111111",
                        "username": "originaluser",
                        "name": "Original User"
                    }
                }
            }]
        }))
        .unwrap()
    }

    /// A reply tweet
    pub fn reply_tweet() -> Tweet {
        serde_json::from_value(json!({
            "id": "1234567890123456792",
            "text": "I agree with this point!",
            "created_at": "2023-01-15T13:00:00.000Z",
            "author_id": "987654321",
            "author": {
                "id": "987654321",
                "username": "replier",
                "name": "Reply User"
            },
            "referenced_tweets": [{
                "type": "replied_to",
                "id": "1234567890123456600",
                "data": {
                    "id": "1234567890123456600",
                    "text": "Here's an interesting observation about the current state of technology.",
                    "created_at": "2023-01-15T08:00:00.000Z",
                    "author_id": "222222222",
                    "author": {
                        "id": "222222222",
                        "username": "opuser",
                        "name": "Original Poster"
                    }
                }
            }]
        }))
        .unwrap()
    }

    /// A quoted tweet
    pub fn quoted_tweet() -> Tweet {
        serde_json::from_value(json!({
            "id": "1234567890123456793",
            "text": "This is worth sharing:",
            "created_at": "2023-01-15T14:00:00.000Z",
            "author_id": "987654321",
            "author": {
                "id": "987654321",
                "username": "quoter",
                "name": "Quote User"
            },
            "referenced_tweets": [{
                "type": "quoted",
                "id": "1234567890123456500",
                "data": {
                    "id": "1234567890123456500",
                    "text": "Breaking: Important news about recent developments in the tech industry.",
                    "created_at": "2023-01-15T07:00:00.000Z",
                    "author_id": "333333333",
                    "author": {
                        "id": "333333333",
                        "username": "newsaccount",
                        "name": "News Account"
                    }
                }
            }]
        }))
        .unwrap()
    }

    /// A long-form note tweet
    pub fn note_tweet() -> Tweet {
        serde_json::from_value(json!({
            "id": "1234567890123456794",
            "text": "🧵 A thread about Rust programming...",
            "created_at": "2023-01-15T15:00:00.000Z",
            "author_id": "987654321",
            "author": {
                "id": "987654321",
                "username": "rustdev",
                "name": "Rust Developer"
            },
            "note_tweet": {
                "text": "🧵 A thread about Rust programming\n\nRust is a systems programming language that runs blazingly fast, prevents segfaults, and guarantees thread safety. Here's why it's becoming increasingly popular:\n\n1. Memory Safety: Rust's ownership system ensures memory safety without needing a garbage collector. This means you get the performance of C/C++ with the safety guarantees that prevent common bugs.\n\n2. Concurrency: Rust's type system and ownership rules prevent data races at compile time. This makes concurrent programming much safer and easier to reason about.\n\n3. Zero-Cost Abstractions: Rust provides high-level ergonomics without sacrificing low-level control. You can write expressive code that compiles down to efficient machine code.\n\n4. Great Tooling: Cargo (Rust's package manager), rustfmt (code formatter), and clippy (linter) provide an excellent developer experience out of the box.\n\n5. Growing Ecosystem: The Rust ecosystem is rapidly expanding with high-quality libraries for web development, embedded systems, game development, and more.\n\nIf you're interested in learning Rust, I recommend starting with the official Rust Book (available free online) and working through Rustlings exercises. The community is welcoming and helpful!\n\n#RustLang #Programming #SystemsProgramming"
            }
        }))
        .unwrap()
    }

    /// Real douglaz tweet - simple reply (from 2025-06-01)
    pub fn douglaz_simple_reply() -> Tweet {
        serde_json::from_str(r#"{
            "id": "1929266300380967406",
            "text": "@naoTankoOBostil @ancapmilavargas @ojedabtc Bons tempos",
            "author": {
                "id": "3916631",
                "name": "douglaz",
                "username": "douglaz",
                "profile_image_url": "https://pbs.twimg.com/profile_images/1639377321386823680/UVcJ5dbZ_normal.jpg"
            },
            "referenced_tweets": [
                {
                    "id": "1929036772777787461",
                    "type": "replied_to"
                }
            ],
            "attachments": null,
            "created_at": "2025-06-01T19:58:24.000Z",
            "entities": {
                "urls": null,
                "mentions": [
                    {
                        "username": "naoTankoOBostil"
                    },
                    {
                        "username": "ancapmilavargas"
                    },
                    {
                        "username": "ojedabtc"
                    }
                ],
                "hashtags": null
            },
            "includes": {
                "media": null,
                "users": null,
                "tweets": null
            },
            "author_id": "3916631"
        }"#).unwrap()
    }

    /// Real douglaz tweet - with URL expansion (from 2023-04-09)
    pub fn douglaz_tweet_with_url() -> Tweet {
        serde_json::from_str(r#"{
            "id": "1645195402788892674",
            "text": "@unknown_BTC_usr Discordo, descentralização é um número real, principalmente quando você modela o sistema como se fosse um grafo. Veja por exemplo https://t.co/THxQa36439",
            "author": {
                "id": "3916631",
                "name": "douglaz",
                "username": "douglaz",
                "profile_image_url": "https://pbs.twimg.com/profile_images/1639377321386823680/UVcJ5dbZ_normal.jpg"
            },
            "referenced_tweets": [
                {
                    "id": "1645149404083494917",
                    "type": "replied_to"
                }
            ],
            "attachments": null,
            "created_at": "2023-04-09T22:42:04.000Z",
            "entities": {
                "urls": [
                    {
                        "url": "https://t.co/THxQa36439",
                        "expanded_url": "https://en.wikipedia.org/wiki/Clustering_coefficient",
                        "display_url": "en.wikipedia.org/wiki/Clusterin…"
                    }
                ],
                "mentions": [
                    {
                        "username": "unknown_BTC_usr"
                    }
                ],
                "hashtags": null
            },
            "includes": {
                "media": null,
                "users": null,
                "tweets": null
            },
            "author_id": "3916631"
        }"#).unwrap()
    }

    /// Real douglaz tweet - Planet of Apes reply (from 2025-06-01)
    pub fn douglaz_planet_apes_reply() -> Tweet {
        serde_json::from_str(r#"{
            "id": "1929221881929843016",
            "text": "@AMAZlNGNATURE So if we want Planet of Apes to happen, we know what to do...",
            "author": {
                "id": "3916631",
                "name": "douglaz",
                "username": "douglaz",
                "profile_image_url": "https://pbs.twimg.com/profile_images/1639377321386823680/UVcJ5dbZ_normal.jpg"
            },
            "referenced_tweets": [
                {
                    "id": "1929216043643273547",
                    "type": "replied_to"
                }
            ],
            "attachments": null,
            "created_at": "2025-06-01T17:01:54.000Z",
            "entities": {
                "urls": null,
                "mentions": [
                    {
                        "username": "AMAZlNGNATURE"
                    }
                ],
                "hashtags": null
            },
            "includes": {
                "media": null,
                "users": null,
                "tweets": null
            },
            "author_id": "3916631"
        }"#).unwrap()
    }

    /// Real douglaz tweet - retweet with video (from 2025-07-27)
    pub fn douglaz_retweet_with_video() -> Tweet {
        serde_json::from_str(r#"{
            "id": "1949597796660551797",
            "text": "RT @newstart_2024: Frank Zappa, 1981: \"Schools train people to be ignorant... They give you the equipment that you need to be a functional…",
            "author": {
                "id": "3916631",
                "name": "douglaz",
                "username": "douglaz",
                "profile_image_url": "https://pbs.twimg.com/profile_images/1639377321386823680/UVcJ5dbZ_normal.jpg",
                "description": "You may also like npub1yvjmvxh2jx07m945mf2lu4j5kswr0d63n0w6cjddj3vpkw4unp4qjarngj",
                "url": "https://t.co/fNbahsM4CC",
                "entities": {
                    "url": {
                        "urls": [
                            {
                                "url": "https://t.co/fNbahsM4CC",
                                "expanded_url": "https://github.com/douglaz",
                                "display_url": "github.com/douglaz"
                            }
                        ]
                    },
                    "description": null
                }
            },
            "referenced_tweets": [
                {
                    "id": "1949546554764705845",
                    "type": "retweeted",
                    "data": {
                        "id": "1949546554764705845",
                        "text": "Frank Zappa, 1981: \"Schools train people to be ignorant... They give you the equipment that you need to be a functional ignoramus.\"\n\n\"They prepare you to be a usable victim for a military industrial complex that needs manpower.\"\n\n\"As long as you're just smart enough to do a job, https://t.co/JsfJ9Q4ndA",
                        "author": {
                            "id": "1109532876310302721",
                            "name": "Camus",
                            "username": "newstart_2024",
                            "profile_image_url": "https://pbs.twimg.com/profile_images/1917152153237327872/f-qClfGh_normal.jpg",
                            "description": "DM for removals/credit\nNo one saves us but ourselves. No one can and no one may. We ourselves must walk the path.\n― Gautama Buddha",
                            "url": "https://t.co/lCKgLPN1OC",
                            "entities": {
                                "url": {
                                    "urls": [
                                        {
                                            "url": "https://t.co/lCKgLPN1OC",
                                            "expanded_url": "https://linktr.ee/newstart__2024",
                                            "display_url": "linktr.ee/newstart__2024"
                                        }
                                    ]
                                },
                                "description": null
                            }
                        },
                        "referenced_tweets": null,
                        "attachments": {
                            "media_keys": [
                                "13_1949544286111809536"
                            ]
                        },
                        "created_at": "2025-07-27T19:04:54.000Z",
                        "entities": {
                            "urls": [
                                {
                                    "url": "https://t.co/JsfJ9Q4ndA",
                                    "expanded_url": "https://x.com/newstart_2024/status/1949546554764705845/video/1",
                                    "display_url": "pic.x.com/JsfJ9Q4ndA"
                                }
                            ],
                            "mentions": null,
                            "hashtags": null
                        },
                        "includes": {
                            "media": [
                                {
                                    "media_key": "13_1949544286111809536",
                                    "type": "video",
                                    "url": null,
                                    "preview_image_url": "https://pbs.twimg.com/amplify_video_thumb/1949544286111809536/img/RNGcNb5F_P0ZEjCH.jpg",
                                    "alt_text": null,
                                    "variants": [
                                        {
                                            "bit_rate": 256000,
                                            "content_type": "video/mp4",
                                            "url": "https://video.twimg.com/amplify_video/1949544286111809536/vid/avc1/368x270/OlQs7r9QATt3w1mN.mp4"
                                        },
                                        {
                                            "bit_rate": 832000,
                                            "content_type": "video/mp4",
                                            "url": "https://video.twimg.com/amplify_video/1949544286111809536/vid/avc1/480x352/JFYPmmXfT3EQV8aE.mp4"
                                        },
                                        {
                                            "bit_rate": null,
                                            "content_type": "application/x-mpegURL",
                                            "url": "https://video.twimg.com/amplify_video/1949544286111809536/pl/INTWkva2FMqH9yG2.m3u8?v=865"
                                        }
                                    ]
                                }
                            ],
                            "users": [
                                {
                                    "id": "1109532876310302721",
                                    "name": "Camus",
                                    "username": "newstart_2024",
                                    "profile_image_url": "https://pbs.twimg.com/profile_images/1917152153237327872/f-qClfGh_normal.jpg",
                                    "description": "DM for removals/credit\nNo one saves us but ourselves. No one can and no one may. We ourselves must walk the path.\n― Gautama Buddha",
                                    "url": "https://t.co/lCKgLPN1OC",
                                    "entities": {
                                        "url": {
                                            "urls": [
                                                {
                                                    "url": "https://t.co/lCKgLPN1OC",
                                                    "expanded_url": "https://linktr.ee/newstart__2024",
                                                    "display_url": "linktr.ee/newstart__2024"
                                                }
                                            ]
                                        },
                                        "description": null
                                    }
                                }
                            ],
                            "tweets": null
                        },
                        "author_id": "1109532876310302721",
                        "note_tweet": {
                            "text": "Frank Zappa, 1981: \"Schools train people to be ignorant... They give you the equipment that you need to be a functional ignoramus.\"\n\n\"They prepare you to be a usable victim for a military industrial complex that needs manpower.\"\n\n\"As long as you're just smart enough to do a job, and just dumb enough to swallow what they feed you, you're going to be alright.\"\n\n\"Schools mechanically and very specifically try and breed out any hint of creative thought in the kids that are coming up.\""
                        }
                    }
                }
            ],
            "attachments": null,
            "created_at": "2025-07-27T22:28:31.000Z",
            "entities": {
                "urls": null,
                "mentions": [
                    {
                        "username": "newstart_2024"
                    }
                ],
                "hashtags": null
            },
            "includes": {
                "media": [
                    {
                        "media_key": "13_1949544286111809536",
                        "type": "video",
                        "url": null,
                        "preview_image_url": "https://pbs.twimg.com/amplify_video_thumb/1949544286111809536/img/RNGcNb5F_P0ZEjCH.jpg",
                        "alt_text": null,
                        "variants": [
                            {
                                "bit_rate": 256000,
                                "content_type": "video/mp4",
                                "url": "https://video.twimg.com/amplify_video/1949544286111809536/vid/avc1/368x270/OlQs7r9QATt3w1mN.mp4"
                            },
                            {
                                "bit_rate": 832000,
                                "content_type": "video/mp4",
                                "url": "https://video.twimg.com/amplify_video/1949544286111809536/vid/avc1/480x352/JFYPmmXfT3EQV8aE.mp4"
                            },
                            {
                                "bit_rate": null,
                                "content_type": "application/x-mpegURL",
                                "url": "https://video.twimg.com/amplify_video/1949544286111809536/pl/INTWkva2FMqH9yG2.m3u8?v=865"
                            }
                        ]
                    }
                ],
                "users": [
                    {
                        "id": "3916631",
                        "name": "douglaz",
                        "username": "douglaz",
                        "profile_image_url": "https://pbs.twimg.com/profile_images/1639377321386823680/UVcJ5dbZ_normal.jpg",
                        "description": "You may also like npub1yvjmvxh2jx07m945mf2lu4j5kswr0d63n0w6cjddj3vpkw4unp4qjarngj",
                        "url": "https://t.co/fNbahsM4CC",
                        "entities": {
                            "url": {
                                "urls": [
                                    {
                                        "url": "https://t.co/fNbahsM4CC",
                                        "expanded_url": "https://github.com/douglaz",
                                        "display_url": "github.com/douglaz"
                                    }
                                ]
                            },
                            "description": null
                        }
                    }
                ],
                "tweets": [
                    {
                        "id": "1949546554764705845",
                        "text": "Frank Zappa, 1981: \"Schools train people to be ignorant... They give you the equipment that you need to be a functional ignoramus.\"\n\n\"They prepare you to be a usable victim for a military industrial complex that needs manpower.\"\n\n\"As long as you're just smart enough to do a job, https://t.co/JsfJ9Q4ndA",
                        "author": {
                            "id": "",
                            "name": null,
                            "username": "",
                            "profile_image_url": null,
                            "description": null,
                            "url": null,
                            "entities": null
                        },
                        "referenced_tweets": null,
                        "attachments": {
                            "media_keys": [
                                "13_1949544286111809536"
                            ]
                        },
                        "created_at": "2025-07-27T19:04:54.000Z",
                        "entities": {
                            "urls": [
                                {
                                    "url": "https://t.co/JsfJ9Q4ndA",
                                    "expanded_url": "https://x.com/newstart_2024/status/1949546554764705845/video/1",
                                    "display_url": "pic.x.com/JsfJ9Q4ndA"
                                }
                            ],
                            "mentions": null,
                            "hashtags": null
                        },
                        "includes": null,
                        "author_id": "1109532876310302721",
                        "note_tweet": {
                            "text": "Frank Zappa, 1981: \"Schools train people to be ignorant... They give you the equipment that you need to be a functional ignoramus.\"\n\n\"They prepare you to be a usable victim for a military industrial complex that needs manpower.\"\n\n\"As long as you're just smart enough to do a job, and just dumb enough to swallow what they feed you, you're going to be alright.\"\n\n\"Schools mechanically and very specifically try and breed out any hint of creative thought in the kids that are coming up.\""
                        }
                    }
                ]
            },
            "author_id": "3916631"
        }"#).unwrap()
    }

    /// Real douglaz tweet - Cannes reply with media in referenced tweet (from 2025-07-30)
    pub fn douglaz_cannes_reply_with_media() -> Tweet {
        serde_json::from_str(r#"{
            "id": "1950547609602433299",
            "text": "@dr_orlovsky Cannes is lovely this time of year",
            "author": {
                "id": "3916631",
                "name": "douglaz",
                "username": "douglaz",
                "profile_image_url": "https://pbs.twimg.com/profile_images/1639377321386823680/UVcJ5dbZ_normal.jpg",
                "description": "You may also like npub1yvjmvxh2jx07m945mf2lu4j5kswr0d63n0w6cjddj3vpkw4unp4qjarngj",
                "url": "https://t.co/fNbahsM4CC",
                "entities": {
                    "url": {
                        "urls": [
                            {
                                "url": "https://t.co/fNbahsM4CC",
                                "expanded_url": "https://github.com/douglaz",
                                "display_url": "github.com/douglaz"
                            }
                        ]
                    },
                    "description": null
                }
            },
            "referenced_tweets": [
                {
                    "id": "1950211658279759953",
                    "type": "replied_to",
                    "data": {
                        "id": "1950211658279759953",
                        "text": "Gradually sinking into a depression. I mean of course as an MD being quite good at clinical psychiatry I can manage them (I mean my depressions) - but sometimes managing depression is accepting it for a while.\n\nStill, fighting it as much as I could (see the picture). https://t.co/FfDMcmpoQ2",
                        "author": {
                            "id": "90660251",
                            "name": "Maxim Orlovsky",
                            "username": "dr_orlovsky",
                            "profile_image_url": "https://pbs.twimg.com/profile_images/1769400930208829440/76SjVZbM_normal.jpg",
                            "description": "Ex Tenebrae sententia: sapiens dominabitur astris. Computer and neuro-scientist, cypherpunk, posthumanist. #AI #robotics, vertical progress. #RGB creator",
                            "url": "https://t.co/DoFzj1mIv2",
                            "entities": {
                                "url": {
                                    "urls": [
                                        {
                                            "url": "https://t.co/DoFzj1mIv2",
                                            "expanded_url": "https://dr.orlovsky.ch",
                                            "display_url": "dr.orlovsky.ch"
                                        }
                                    ]
                                },
                                "description": {
                                    "urls": null,
                                    "mentions": null,
                                    "hashtags": [
                                        {
                                            "tag": "AI"
                                        },
                                        {
                                            "tag": "robotics"
                                        },
                                        {
                                            "tag": "RGB"
                                        }
                                    ]
                                }
                            }
                        },
                        "referenced_tweets": null,
                        "attachments": {
                            "media_keys": [
                                "3_1950211651019354113"
                            ]
                        },
                        "created_at": "2025-07-29T15:07:47.000Z",
                        "entities": {
                            "urls": [
                                {
                                    "url": "https://t.co/FfDMcmpoQ2",
                                    "expanded_url": "https://x.com/dr_orlovsky/status/1950211658279759953/photo/1",
                                    "display_url": "pic.x.com/FfDMcmpoQ2"
                                }
                            ],
                            "mentions": null,
                            "hashtags": null
                        },
                        "includes": {
                            "media": [
                                {
                                    "media_key": "3_1950211651019354113",
                                    "type": "photo",
                                    "url": "https://pbs.twimg.com/media/GxCLKffWMAEBc5X.jpg",
                                    "preview_image_url": null,
                                    "alt_text": null,
                                    "variants": null
                                }
                            ],
                            "users": [
                                {
                                    "id": "90660251",
                                    "name": "Maxim Orlovsky",
                                    "username": "dr_orlovsky",
                                    "profile_image_url": "https://pbs.twimg.com/profile_images/1769400930208829440/76SjVZbM_normal.jpg",
                                    "description": "Ex Tenebrae sententia: sapiens dominabitur astris. Computer and neuro-scientist, cypherpunk, posthumanist. #AI #robotics, vertical progress. #RGB creator",
                                    "url": "https://t.co/DoFzj1mIv2",
                                    "entities": {
                                        "url": {
                                            "urls": [
                                                {
                                                    "url": "https://t.co/DoFzj1mIv2",
                                                    "expanded_url": "https://dr.orlovsky.ch",
                                                    "display_url": "dr.orlovsky.ch"
                                                }
                                            ]
                                        },
                                        "description": {
                                            "urls": null,
                                            "mentions": null,
                                            "hashtags": [
                                                {
                                                    "tag": "AI"
                                                },
                                                {
                                                    "tag": "robotics"
                                                },
                                                {
                                                    "tag": "RGB"
                                                }
                                            ]
                                        }
                                    }
                                }
                            ],
                            "tweets": null
                        },
                        "author_id": "90660251"
                    }
                }
            ],
            "attachments": null,
            "created_at": "2025-07-30T13:22:44.000Z",
            "entities": {
                "urls": null,
                "mentions": [
                    {
                        "username": "dr_orlovsky"
                    }
                ],
                "hashtags": null
            },
            "includes": {
                "media": [
                    {
                        "media_key": "3_1950211651019354113",
                        "type": "photo",
                        "url": "https://pbs.twimg.com/media/GxCLKffWMAEBc5X.jpg",
                        "preview_image_url": null,
                        "alt_text": null,
                        "variants": null
                    }
                ],
                "users": [
                    {
                        "id": "3916631",
                        "name": "douglaz",
                        "username": "douglaz",
                        "profile_image_url": "https://pbs.twimg.com/profile_images/1639377321386823680/UVcJ5dbZ_normal.jpg",
                        "description": "You may also like npub1yvjmvxh2jx07m945mf2lu4j5kswr0d63n0w6cjddj3vpkw4unp4qjarngj",
                        "url": "https://t.co/fNbahsM4CC",
                        "entities": {
                            "url": {
                                "urls": [
                                    {
                                        "url": "https://t.co/fNbahsM4CC",
                                        "expanded_url": "https://github.com/douglaz",
                                        "display_url": "github.com/douglaz"
                                    }
                                ]
                            },
                            "description": null
                        }
                    }
                ],
                "tweets": [
                    {
                        "id": "1950211658279759953",
                        "text": "Gradually sinking into a depression. I mean of course as an MD being quite good at clinical psychiatry I can manage them (I mean my depressions) - but sometimes managing depression is accepting it for a while.\n\nStill, fighting it as much as I could (see the picture). https://t.co/FfDMcmpoQ2",
                        "author": {
                            "id": "",
                            "name": null,
                            "username": "",
                            "profile_image_url": null,
                            "description": null,
                            "url": null,
                            "entities": null
                        },
                        "referenced_tweets": null,
                        "attachments": {
                            "media_keys": [
                                "3_1950211651019354113"
                            ]
                        },
                        "created_at": "2025-07-29T15:07:47.000Z",
                        "entities": {
                            "urls": [
                                {
                                    "url": "https://t.co/FfDMcmpoQ2",
                                    "expanded_url": "https://x.com/dr_orlovsky/status/1950211658279759953/photo/1",
                                    "display_url": "pic.x.com/FfDMcmpoQ2"
                                }
                            ],
                            "mentions": null,
                            "hashtags": null
                        },
                        "includes": null,
                        "author_id": "90660251"
                    }
                ],
                "author_id": "3916631"
            }
        }"#).unwrap()
    }

    /// A retweet that previously showed only URLs - regression test for issue where
    /// retweets were showing URLs instead of full content
    pub fn retweet_with_full_content() -> Tweet {
        serde_json::from_value(json!({
            "id": "1961875988859535529",
            "text": r#"RT @MedicoLiberdade: Quando falo isso o povo me chama de radical, mas até de "amigos" que são de esquerda me afastei. Isso realmente muda o…"#,
            "created_at": "2025-08-30T19:37:40.000Z",
            "author_id": "3916631",
            "author": {
                "id": "3916631",
                "username": "douglaz",
                "name": "douglaz"
            },
            "referenced_tweets": [{
                "type": "retweeted",
                "id": "1961747503176356209",
                "data": {
                    "id": "1961747503176356209",
                    "text": r#"Quando falo isso o povo me chama de radical, mas até de "amigos" que são de esquerda me afastei. Isso realmente muda o seu dia a dia, o seu entorno, o seu habito. Certíssima ela.  https://t.co/sxDlqKciKK"#,
                    "created_at": "2025-08-30T11:07:06.000Z",
                    "author_id": "1591429677930905601",
                    "author": {
                        "id": "1591429677930905601",
                        "username": "MedicoLiberdade",
                        "name": "Médicos Pela Liberdade"
                    },
                    "entities": {
                        "urls": [{
                            "url": "https://t.co/sxDlqKciKK",
                            "expanded_url": "https://x.com/paulodetarsog/status/1961205947709153594/video/1",
                            "display_url": "pic.x.com/sxDlqKciKK"
                        }]
                    },
                    "includes": {
                        "media": [{
                            "media_key": "7_1961205918026145792",
                            "type": "video",
                            "url": null,
                            "preview_image_url": "https://pbs.twimg.com/ext_tw_video_thumb/1961205918026145792/pu/img/wXL9thaP041tzKSf.jpg",
                            "variants": [{
                                "bit_rate": 2176000,
                                "content_type": "video/mp4",
                                "url": "https://video.twimg.com/ext_tw_video/1961205918026145792/pu/vid/avc1/720x900/Mmc-zSbPPpZX1a8o.mp4?tag=12"
                            }]
                        }]
                    }
                }
            }]
        }))
        .unwrap()
    }
}

#[test]
fn test_parse_simple_tweet() -> anyhow::Result<()> {
    let tweet = fixtures::simple_tweet();
    pretty_assertions::assert_eq!(tweet.id, "1234567890123456789");
    pretty_assertions::assert_eq!(tweet.text, "Hello Twitter! This is a test tweet.");
    pretty_assertions::assert_eq!(tweet.author.username, "testuser");
    Ok(())
}

#[test]
fn test_parse_tweet_with_url() -> anyhow::Result<()> {
    let tweet = fixtures::tweet_with_url();
    assert!(tweet.entities.is_some());
    let entities = tweet
        .entities
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("entities missing"))?;
    assert!(entities.urls.is_some());
    let urls = entities
        .urls
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("urls missing"))?;
    pretty_assertions::assert_eq!(urls.len(), 1);
    pretty_assertions::assert_eq!(
        urls[0].expanded_url.as_deref(),
        Some("https://example.com/interesting-article")
    );
    Ok(())
}

#[test]
fn test_format_simple_tweet_nostr() {
    let tweet = fixtures::simple_tweet();
    let content = format_tweet_as_nostr_content(&tweet, &[]);

    assert!(content.contains("🐦 @testuser:"));
    assert!(content.contains("Hello Twitter! This is a test tweet."));
    // Check for the twitter status URL in the content
    assert!(content.contains("Original tweet: https://twitter.com/i/status/1234567890123456789"));
}

#[test]
fn test_format_tweet_with_url_nostr() {
    let tweet = fixtures::tweet_with_url();
    let content = format_tweet_as_nostr_content(&tweet, &[]);

    assert!(content.contains("🐦 @testuser:"));
    assert!(content.contains("Check out this article: [example.com/interesting-ar...](https://example.com/interesting-article)"));
    assert!(!content.contains("https://t.co/abc123def")); // Should be replaced
}

#[test]
fn test_format_retweet_nostr() {
    let tweet = fixtures::retweet();
    let content = format_tweet_as_nostr_content(&tweet, &[]);

    assert!(content.contains("🔁 @retweeter retweeted @originaluser:"));
    assert!(content.contains("This is the original tweet content that was retweeted"));
    assert!(content.contains("https://twitter.com/i/status/1234567890123456700"));
}

#[test]
fn test_format_retweet_with_full_content() {
    // Regression test: ensure retweets show full content, not just URLs
    let tweet = fixtures::retweet_with_full_content();
    let content = format_tweet_as_nostr_content(&tweet, &[]);

    // Should be formatted as a retweet
    assert!(content.contains("🔁 @douglaz retweeted @MedicoLiberdade:"));

    // Should contain the full text of the retweeted content
    assert!(
        content.contains("Quando falo isso o povo me chama de radical"),
        "Retweet should contain the full text content, not just URLs"
    );
    assert!(
        content.contains("são de esquerda me afastei"),
        "Retweet should contain middle part of the text"
    );
    assert!(
        content.contains("Certíssima ela"),
        "Retweet should contain end of the text"
    );

    // Should also have the tweet link
    assert!(content.contains("https://twitter.com/i/status/1961747503176356209"));

    // Regression test: t.co URLs should be expanded to actual media URLs
    assert!(
        !content.contains("https://t.co/sxDlqKciKK"),
        "t.co URL should be expanded, not shown as-is"
    );
    // Check that the video URL is present (either direct twimg URL or expanded Twitter URL)
    assert!(
        content.contains("video.twimg.com") || content.contains("/video/1"),
        "Video URL should be expanded inline in the text (either direct or Twitter URL)"
    );
}

#[test]
fn test_daemon_retweet_formatting_with_mentions() {
    use nostrweet::nostr::format_tweet_as_nostr_content_with_mentions;
    use nostrweet::nostr_linking::NostrLinkResolver;

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    // Regression test: ensure daemon's retweet formatting (which uses format_retweet_with_mentions)
    // properly extracts media URLs and doesn't show t.co URLs
    let tweet = fixtures::retweet_with_full_content();
    let mut resolver = NostrLinkResolver::new(None, Some(TEST_MNEMONIC.to_string()));

    let result = format_tweet_as_nostr_content_with_mentions(&tweet, &[], &mut resolver);
    if let Err(e) = &result {
        panic!("Daemon formatting failed: {e}");
    }

    let (content, _mentioned_pubkeys) = result.unwrap();

    // Should be formatted as a retweet with daemon's Nostr-aware format
    // The daemon resolves usernames to Nostr pubkeys when possible
    assert!(content.contains("🔁 @douglaz retweeted nostr:npub"));

    // Should contain the full text of the retweeted content
    assert!(
        content.contains("Quando falo isso o povo me chama de radical"),
        "Daemon retweet should contain the full text content, not just URLs"
    );

    // Critical regression test: daemon should not show t.co URLs
    assert!(
        !content.contains("https://t.co/sxDlqKciKK"),
        "Daemon should expand t.co URLs, not show them as-is"
    );
    assert!(
        content.contains("video.twimg.com"),
        "Daemon should show expanded video URL inline in the text"
    );
}

#[test]
fn test_format_reply_nostr() {
    let tweet = fixtures::reply_tweet();
    let content = format_tweet_as_nostr_content(&tweet, &[]);

    assert!(content.contains("🐦 @replier:"));
    assert!(content.contains("I agree with this point!"));
    assert!(content.contains("↩️ Reply to @opuser:"));
    assert!(content.contains("Here's an interesting observation"));
}

#[test]
fn test_format_quoted_tweet_nostr() {
    let tweet = fixtures::quoted_tweet();
    let content = format_tweet_as_nostr_content(&tweet, &[]);

    assert!(content.contains("🐦 @quoter:"));
    assert!(content.contains("This is worth sharing:"));
    assert!(content.contains("💬 Quote of @newsaccount:"));
    assert!(content.contains("Breaking: Important news"));
}

#[test]
fn test_format_note_tweet_nostr() {
    let tweet = fixtures::note_tweet();
    let content = format_tweet_as_nostr_content(&tweet, &[]);

    assert!(content.contains("🐦 @rustdev:"));
    // Should contain the full note_tweet text, not the truncated version
    assert!(content.contains("Rust is a systems programming language"));
    assert!(content.contains("Memory Safety:"));
    assert!(content.contains("Concurrency:"));
    assert!(content.contains("#RustLang #Programming"));
}

#[test]
fn test_tweet_with_media_urls() {
    let tweet = fixtures::simple_tweet();
    let media_urls = vec![
        "https://blossom.example.com/sha256/abc123.jpg".to_string(),
        "https://blossom.example.com/sha256/def456.mp4".to_string(),
    ];
    let content = format_tweet_as_nostr_content(&tweet, &media_urls);

    assert!(content.contains("https://blossom.example.com/sha256/abc123.jpg"));
    assert!(content.contains("https://blossom.example.com/sha256/def456.mp4"));
}

/// Test that formatting is consistent and predictable
#[test]
fn test_format_consistency() {
    let tweet = fixtures::simple_tweet();
    let content1 = format_tweet_as_nostr_content(&tweet, &[]);
    let content2 = format_tweet_as_nostr_content(&tweet, &[]);

    // Same input should produce same output
    pretty_assertions::assert_eq!(content1, content2);
}

/// Test edge cases
#[test]
fn test_empty_username() {
    let mut tweet = fixtures::simple_tweet();
    tweet.author.username = String::new();
    let content = format_tweet_as_nostr_content(&tweet, &[]);

    // Should fall back to user ID
    assert!(content.contains("🐦 User 987654321:"));
}

#[test]
fn test_missing_author_id() {
    let mut tweet = fixtures::simple_tweet();
    tweet.author.username = String::new();
    tweet.author.id = String::new(); // Clear this too
    tweet.author_id = None;
    let content = format_tweet_as_nostr_content(&tweet, &[]);

    // Should fall back to generic "Tweet:"
    assert!(content.contains("🐦 Tweet:"));
}

/// Helper function to create tags for a nostr event
fn create_nostr_event_tags(
    tweet_id: &str,
    media_urls: &[String],
) -> Result<Vec<Tag>, Box<dyn std::error::Error>> {
    let mut tags = Vec::new();

    // Add tweet reference tag
    let twitter_url = format!("https://twitter.com/i/status/{tweet_id}");
    tags.push(Tag::parse(vec!["r", &twitter_url])?);

    // Add media tags if present
    for url in media_urls {
        tags.push(Tag::parse(vec!["r", url])?);
    }

    Ok(tags)
}

/// Helper function to create a complete Nostr event from a tweet
async fn create_nostr_event_from_tweet(
    tweet: &Tweet,
    media_urls: &[String],
    keys: &Keys,
) -> Result<nostr_sdk::Event, Box<dyn std::error::Error>> {
    let content = format_tweet_as_nostr_content(tweet, media_urls);

    // Parse the created_at timestamp
    let timestamp = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&tweet.created_at) {
        Timestamp::from(dt.timestamp() as u64)
    } else {
        // Fallback to current time
        Timestamp::from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
    };

    // Create tags
    let tags = create_nostr_event_tags(&tweet.id, media_urls)?;

    // Create event builder
    let mut builder = EventBuilder::new(Kind::TextNote, content).custom_created_at(timestamp);

    // Add tags
    for tag in tags {
        builder = builder.tag(tag);
    }

    // Sign the event
    let event = builder.sign(keys).await?;

    Ok(event)
}

// Comprehensive regression tests with real douglaz tweets

#[tokio::test]
async fn test_douglaz_simple_reply_full_nostr_event() {
    let tweet = fixtures::douglaz_simple_reply();
    let keys = Keys::generate();
    let media_urls = vec![];

    let event = create_nostr_event_from_tweet(&tweet, &media_urls, &keys)
        .await
        .expect("Failed to create Nostr event");

    // Verify event structure
    pretty_assertions::assert_eq!(event.kind, Kind::TextNote);
    pretty_assertions::assert_eq!(event.pubkey, keys.public_key());

    // Verify content formatting
    let content = &event.content;
    assert!(content.contains("🐦 @douglaz:"));
    assert!(content.contains("@naoTankoOBostil @ancapmilavargas @ojedabtc Bons tempos"));
    assert!(content.contains("Original tweet: https://twitter.com/i/status/1929266300380967406"));

    // Verify tags
    let has_twitter_ref = event.tags.iter().any(|tag| {
        let tag_vec = (*tag).clone().to_vec();
        tag_vec.len() >= 2 && tag_vec[0] == "r" && tag_vec[1].contains("status/1929266300380967406")
    });
    assert!(has_twitter_ref, "Event should have Twitter reference tag");

    // Verify timestamp (should match tweet creation time)
    let expected_timestamp = chrono::DateTime::parse_from_rfc3339("2025-06-01T19:58:24.000Z")
        .unwrap()
        .timestamp() as u64;
    pretty_assertions::assert_eq!(event.created_at.as_u64(), expected_timestamp);

    // Verify the event can be serialized/deserialized
    let json = serde_json::to_string(&event).expect("Failed to serialize event");
    let _deserialized: nostr_sdk::Event =
        serde_json::from_str(&json).expect("Failed to deserialize event");
}

#[tokio::test]
async fn test_douglaz_url_expansion_full_nostr_event() {
    let tweet = fixtures::douglaz_tweet_with_url();
    let keys = Keys::generate();
    let media_urls = vec![];

    let event = create_nostr_event_from_tweet(&tweet, &media_urls, &keys)
        .await
        .expect("Failed to create Nostr event");

    // Verify event structure
    pretty_assertions::assert_eq!(event.kind, Kind::TextNote);
    pretty_assertions::assert_eq!(event.pubkey, keys.public_key());

    // Verify content formatting with URL expansion
    let content = &event.content;
    assert!(content.contains("🐦 @douglaz:"));
    assert!(content.contains("@unknown_BTC_usr Discordo, descentralização é um número real"));
    assert!(content.contains(
        "[en.wikipedia.org/wiki/Clusterin…](https://en.wikipedia.org/wiki/Clustering_coefficient)"
    ));
    assert!(!content.contains("https://t.co/THxQa36439")); // Should be expanded
    assert!(content.contains("Original tweet: https://twitter.com/i/status/1645195402788892674"));

    // Verify tags
    let has_twitter_ref = event.tags.iter().any(|tag| {
        let tag_vec = (*tag).clone().to_vec();
        tag_vec.len() >= 2 && tag_vec[0] == "r" && tag_vec[1].contains("status/1645195402788892674")
    });
    assert!(has_twitter_ref, "Event should have Twitter reference tag");

    // Verify timestamp matches tweet creation time
    let expected_timestamp = chrono::DateTime::parse_from_rfc3339("2023-04-09T22:42:04.000Z")
        .unwrap()
        .timestamp() as u64;
    pretty_assertions::assert_eq!(event.created_at.as_u64(), expected_timestamp);

    // Verify the event is deterministic - same tweet should produce same content
    let event2 = create_nostr_event_from_tweet(&tweet, &media_urls, &keys)
        .await
        .expect("Failed to create second Nostr event");
    pretty_assertions::assert_eq!(event.content, event2.content);
    pretty_assertions::assert_eq!(event.created_at, event2.created_at);
}

#[tokio::test]
async fn test_douglaz_planet_apes_reply_full_nostr_event() {
    let tweet = fixtures::douglaz_planet_apes_reply();
    let keys = Keys::generate();
    let media_urls = vec![];

    let event = create_nostr_event_from_tweet(&tweet, &media_urls, &keys)
        .await
        .expect("Failed to create Nostr event");

    // Verify event structure
    pretty_assertions::assert_eq!(event.kind, Kind::TextNote);
    pretty_assertions::assert_eq!(event.pubkey, keys.public_key());

    // Verify content formatting
    let content = &event.content;
    assert!(content.contains("🐦 @douglaz:"));
    assert!(
        content.contains(
            "@AMAZlNGNATURE So if we want Planet of Apes to happen, we know what to do..."
        )
    );
    assert!(content.contains("Original tweet: https://twitter.com/i/status/1929221881929843016"));

    // Verify tags
    let has_twitter_ref = event.tags.iter().any(|tag| {
        let tag_vec = (*tag).clone().to_vec();
        tag_vec.len() >= 2 && tag_vec[0] == "r" && tag_vec[1].contains("status/1929221881929843016")
    });
    assert!(has_twitter_ref, "Event should have Twitter reference tag");

    // Verify timestamp
    let expected_timestamp = chrono::DateTime::parse_from_rfc3339("2025-06-01T17:01:54.000Z")
        .unwrap()
        .timestamp() as u64;
    pretty_assertions::assert_eq!(event.created_at.as_u64(), expected_timestamp);

    // Verify event has proper structure for a reply
    let json = serde_json::to_string_pretty(&event).expect("Failed to serialize event");
    // The JSON representation uses numeric values for Kind, so check for 1 (TextNote)
    assert!(json.contains("\"kind\": 1") || json.contains("\"kind\":1"));

    // Verify the event can be round-tripped through JSON
    let _deserialized: nostr_sdk::Event =
        serde_json::from_str(&json).expect("Failed to deserialize event");
}

#[tokio::test]
async fn test_nostr_event_with_media_urls() {
    let tweet = fixtures::douglaz_simple_reply();
    let keys = Keys::generate();
    let media_urls = vec![
        "https://blossom.example.com/sha256/abc123.jpg".to_string(),
        "https://blossom.example.com/sha256/def456.mp4".to_string(),
    ];

    let event = create_nostr_event_from_tweet(&tweet, &media_urls, &keys)
        .await
        .expect("Failed to create Nostr event");

    // Verify content includes media URLs
    let content = &event.content;
    assert!(content.contains("https://blossom.example.com/sha256/abc123.jpg"));
    assert!(content.contains("https://blossom.example.com/sha256/def456.mp4"));

    // Verify tags include media references
    let has_image_ref = event.tags.iter().any(|tag| {
        let tag_vec = (*tag).clone().to_vec();
        tag_vec.len() >= 2 && tag_vec[0] == "r" && tag_vec[1].contains("abc123.jpg")
    });
    let has_video_ref = event.tags.iter().any(|tag| {
        let tag_vec = (*tag).clone().to_vec();
        tag_vec.len() >= 2 && tag_vec[0] == "r" && tag_vec[1].contains("def456.mp4")
    });
    assert!(has_image_ref, "Event should have image reference tag");
    assert!(has_video_ref, "Event should have video reference tag");

    // Should have both media tags plus the twitter reference tag
    let r_tags_count = event
        .tags
        .iter()
        .filter(|tag| {
            let tag_vec = (*tag).clone().to_vec();
            tag_vec.len() >= 2 && tag_vec[0] == "r"
        })
        .count();
    pretty_assertions::assert_eq!(r_tags_count, 3); // 2 media + 1 twitter reference
}

#[tokio::test]
async fn test_nostr_event_deterministic_creation() {
    let tweet = fixtures::douglaz_tweet_with_url();
    let keys = Keys::generate();
    let media_urls = vec![];

    // Create the same event multiple times
    let event1 = create_nostr_event_from_tweet(&tweet, &media_urls, &keys)
        .await
        .expect("Failed to create first event");

    let event2 = create_nostr_event_from_tweet(&tweet, &media_urls, &keys)
        .await
        .expect("Failed to create second event");

    // Content and timestamp should be identical (deterministic based on tweet data)
    pretty_assertions::assert_eq!(event1.content, event2.content);
    pretty_assertions::assert_eq!(event1.created_at, event2.created_at);
    pretty_assertions::assert_eq!(event1.kind, event2.kind);

    // But event IDs will be different because they include the signature
    // (since we're using the same keys, the signature depends on a random nonce)
    // However, the base event structure should be the same

    // Verify both events serialize to valid JSON
    let json1 = serde_json::to_string_pretty(&event1).expect("Failed to serialize event1");
    let json2 = serde_json::to_string_pretty(&event2).expect("Failed to serialize event2");

    // Both should be valid Nostr events
    let _: nostr_sdk::Event = serde_json::from_str(&json1).expect("Invalid event1 JSON");
    let _: nostr_sdk::Event = serde_json::from_str(&json2).expect("Invalid event2 JSON");
}

// Regression tests for show-tweet command

#[tokio::test]
async fn test_show_tweet_output_separation() {
    use serde_json::Value;
    use std::process::Command;
    use tempfile::TempDir;

    // Create a temporary directory for this test
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    // Save a test tweet to the temp directory first
    let tweet = fixtures::douglaz_simple_reply();
    let tweet_path = temp_dir
        .path()
        .join("20250601_195824_douglaz_1929266300380967406.json");
    let tweet_json = serde_json::to_string_pretty(&tweet).expect("Failed to serialize tweet");
    std::fs::write(&tweet_path, tweet_json).expect("Failed to write test tweet");

    // Run the show-tweet command and capture both stdout and stderr
    // Using cargo run with --quiet to suppress compilation output
    let output = Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--",
            "show-tweet",
            "1929266300380967406",
            "--data-dir",
            temp_dir.path().to_str().unwrap(),
            "--mnemonic",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ])
        .output()
        .expect("Failed to execute command");

    // Check that the command succeeded
    assert!(
        output.status.success(),
        "Command failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Parse stdout as JSON
    let stdout_str = String::from_utf8(output.stdout).expect("Invalid UTF-8 in stdout");
    let json: Value = serde_json::from_str(&stdout_str).expect("stdout should be valid JSON");

    // Verify the JSON structure has both twitter and nostr keys
    assert!(
        json.get("twitter").is_some(),
        "JSON should have 'twitter' key"
    );
    assert!(json.get("nostr").is_some(), "JSON should have 'nostr' key");

    // Verify twitter data
    let twitter_data = &json["twitter"];
    pretty_assertions::assert_eq!(twitter_data["id"], "1929266300380967406");
    pretty_assertions::assert_eq!(twitter_data["author"]["username"], "douglaz");

    // Verify nostr data
    let nostr_data = &json["nostr"];
    assert!(
        nostr_data.get("event").is_some(),
        "Nostr data should have 'event' key"
    );
    assert!(
        nostr_data.get("metadata").is_some(),
        "Nostr data should have 'metadata' key"
    );

    let nostr_event = &nostr_data["event"];
    pretty_assertions::assert_eq!(nostr_event["kind"], 1); // TextNote kind
    assert!(nostr_event.get("content").is_some());
    assert!(nostr_event.get("pubkey").is_some());
    assert!(nostr_event.get("sig").is_some());
    assert!(nostr_event.get("tags").is_some());

    let nostr_metadata = &nostr_data["metadata"];
    pretty_assertions::assert_eq!(nostr_metadata["original_tweet_id"], "1929266300380967406");
    pretty_assertions::assert_eq!(nostr_metadata["original_author"], "douglaz");

    // Check stderr contains logging messages (convert to string for analysis)
    let stderr_str = String::from_utf8(output.stderr).expect("Invalid UTF-8 in stderr");

    assert!(
        stderr_str.contains("Showing tweet"),
        "stderr should contain log messages"
    );
    // Accept any of these messages that indicate the tweet was found/loaded
    assert!(
        stderr_str.contains("Found existing tweet data")
            || stderr_str.contains("not found locally")
            || stderr_str.contains("Loaded tweet data from local file"),
        "stderr should contain processing messages"
    );
}

#[tokio::test]
async fn test_show_tweet_stdout_is_pure_json() {
    use std::process::Command;
    use tempfile::TempDir;

    // Create a temporary directory for this test
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    // Save a test tweet to the temp directory first
    let tweet = fixtures::douglaz_tweet_with_url();
    let tweet_path = temp_dir
        .path()
        .join("20230409_224204_douglaz_1645195402788892674.json");
    let tweet_json = serde_json::to_string_pretty(&tweet).expect("Failed to serialize tweet");
    std::fs::write(&tweet_path, tweet_json).expect("Failed to write test tweet");

    // Run the show-tweet command and capture stdout only
    let output = Command::new("cargo")
        .args([
            "run",
            "--",
            "show-tweet",
            "1645195402788892674",
            "--data-dir",
            temp_dir.path().to_str().unwrap(),
            "--mnemonic",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ])
        .output()
        .expect("Failed to execute command");

    // Check that the command succeeded
    assert!(output.status.success(), "Command failed");

    let stdout_str = String::from_utf8(output.stdout).expect("Invalid UTF-8 in stdout");

    // Verify stdout contains no log formatting or non-JSON content
    assert!(
        !stdout_str.contains("INFO"),
        "stdout should not contain log level indicators"
    );
    assert!(
        !stdout_str.contains("==="),
        "stdout should not contain section headers"
    );
    assert!(
        !stdout_str.contains("Showing tweet"),
        "stdout should not contain log messages"
    );
    assert!(
        !stdout_str.contains("Found cached"),
        "stdout should not contain progress messages"
    );

    // Verify it's valid JSON by parsing it
    let json: serde_json::Value =
        serde_json::from_str(&stdout_str).expect("stdout should be parseable as JSON");

    // Verify it has the expected top-level structure
    assert!(json.is_object(), "JSON should be an object");
    assert!(json.get("twitter").is_some(), "Should have twitter key");
    assert!(json.get("nostr").is_some(), "Should have nostr key");

    // Verify URL expansion works correctly
    let twitter_text = json["twitter"]["text"].as_str().unwrap();
    assert!(
        twitter_text.contains("https://t.co/THxQa36439"),
        "Original tweet should have t.co URL"
    );

    let nostr_content = json["nostr"]["event"]["content"].as_str().unwrap();
    assert!(
        nostr_content.contains("en.wikipedia.org/wiki/Clustering_coefficient"),
        "Nostr content should have expanded URL"
    );
}

#[tokio::test]
async fn test_show_tweet_pretty_formatting() {
    use std::process::Command;
    use tempfile::TempDir;

    // Create a temporary directory for this test
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    // Save a test tweet to the temp directory first
    let tweet = fixtures::douglaz_planet_apes_reply();
    let tweet_path = temp_dir
        .path()
        .join("20250601_170154_douglaz_1929221881929843016.json");
    let tweet_json = serde_json::to_string_pretty(&tweet).expect("Failed to serialize tweet");
    std::fs::write(&tweet_path, tweet_json).expect("Failed to write test tweet");

    // Test with pretty=true (default)
    let output_pretty = Command::new("cargo")
        .args([
            "run",
            "--",
            "show-tweet",
            "1929221881929843016",
            "--data-dir",
            temp_dir.path().to_str().unwrap(),
            "--pretty",
            "--mnemonic",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output_pretty.status.success(), "Pretty command failed");
    let pretty_stdout = String::from_utf8(output_pretty.stdout).expect("Invalid UTF-8");

    // Pretty output should have indentation and newlines
    assert!(
        pretty_stdout.contains("  "),
        "Pretty output should have indentation"
    );
    assert!(
        pretty_stdout.contains("\n"),
        "Pretty output should have newlines"
    );

    // Test with compact format
    let output_compact = Command::new("cargo")
        .args([
            "run",
            "--",
            "show-tweet",
            "1929221881929843016",
            "--data-dir",
            temp_dir.path().to_str().unwrap(),
            "--compact",
            "--mnemonic",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output_compact.status.success(), "Compact command failed");
    let compact_stdout = String::from_utf8(output_compact.stdout).expect("Invalid UTF-8");

    // Compact output should be a single line
    pretty_assertions::assert_eq!(
        compact_stdout.lines().count(),
        1,
        "Compact output should be single line"
    );

    // Both should parse to the same JSON structure
    let pretty_json: serde_json::Value = serde_json::from_str(&pretty_stdout).unwrap();
    let compact_json: serde_json::Value = serde_json::from_str(&compact_stdout).unwrap();

    // Compare the twitter data (should be identical)
    pretty_assertions::assert_eq!(
        pretty_json["twitter"],
        compact_json["twitter"],
        "Twitter data should be identical"
    );

    // Compare nostr event content and structure (but not cryptographic fields like id, pubkey, sig)
    pretty_assertions::assert_eq!(
        pretty_json["nostr"]["event"]["content"],
        compact_json["nostr"]["event"]["content"],
        "Nostr content should be identical"
    );
    pretty_assertions::assert_eq!(
        pretty_json["nostr"]["event"]["kind"],
        compact_json["nostr"]["event"]["kind"],
        "Nostr kind should be identical"
    );
    pretty_assertions::assert_eq!(
        pretty_json["nostr"]["event"]["created_at"],
        compact_json["nostr"]["event"]["created_at"],
        "Nostr timestamp should be identical"
    );
    pretty_assertions::assert_eq!(
        pretty_json["nostr"]["event"]["tags"],
        compact_json["nostr"]["event"]["tags"],
        "Nostr tags should be identical"
    );

    // Compare metadata (excluding fields that depend on the generated keys)
    pretty_assertions::assert_eq!(
        pretty_json["nostr"]["metadata"]["original_tweet_id"],
        compact_json["nostr"]["metadata"]["original_tweet_id"]
    );
    pretty_assertions::assert_eq!(
        pretty_json["nostr"]["metadata"]["original_author"],
        compact_json["nostr"]["metadata"]["original_author"]
    );
    pretty_assertions::assert_eq!(
        pretty_json["nostr"]["metadata"]["created_at_human"],
        compact_json["nostr"]["metadata"]["created_at_human"]
    );
    pretty_assertions::assert_eq!(
        pretty_json["nostr"]["metadata"]["tags_count"],
        compact_json["nostr"]["metadata"]["tags_count"]
    );
}

#[tokio::test]
async fn test_douglaz_cannes_reply_with_referenced_tweet_media() {
    let tweet = fixtures::douglaz_cannes_reply_with_media();
    let keys = Keys::generate();
    let media_urls = vec![];

    let event = create_nostr_event_from_tweet(&tweet, &media_urls, &keys)
        .await
        .expect("Failed to create Nostr event");

    // Verify event structure
    pretty_assertions::assert_eq!(event.kind, Kind::TextNote);
    pretty_assertions::assert_eq!(event.pubkey, keys.public_key());

    // Verify content formatting
    let content = &event.content;

    // Main tweet content
    assert!(content.contains("🐦 @douglaz:"));
    assert!(content.contains("@dr_orlovsky Cannes is lovely this time of year"));

    // Reply header
    assert!(content.contains("↩️ Reply to @dr_orlovsky:"));

    // Referenced tweet content (should be fully included)
    assert!(content.contains("Gradually sinking into a depression"));
    assert!(content.contains("I mean of course as an MD being quite good at clinical psychiatry"));
    assert!(content.contains("Still, fighting it as much as I could"));

    // Media URL from referenced tweet should be expanded inline
    assert!(
        content.contains("https://pbs.twimg.com/media/GxCLKffWMAEBc5X.jpg"),
        "Referenced tweet's media URL should be included inline"
    );

    // t.co URL should NOT be present (it should be replaced with actual media)
    assert!(
        !content.contains("https://t.co/FfDMcmpoQ2"),
        "t.co URL should be replaced with actual media URL"
    );

    // Referenced tweet link
    assert!(content.contains("https://twitter.com/i/status/1950211658279759953"));

    // Original tweet link
    assert!(content.contains("Original tweet: https://twitter.com/i/status/1950547609602433299"));

    // Verify tags
    let has_twitter_ref = event.tags.iter().any(|tag| {
        let tag_vec = (*tag).clone().to_vec();
        tag_vec.len() >= 2 && tag_vec[0] == "r" && tag_vec[1].contains("status/1950547609602433299")
    });
    assert!(has_twitter_ref, "Event should have Twitter reference tag");

    // Verify timestamp
    let expected_timestamp = chrono::DateTime::parse_from_rfc3339("2025-07-30T13:22:44.000Z")
        .unwrap()
        .timestamp() as u64;
    pretty_assertions::assert_eq!(event.created_at.as_u64(), expected_timestamp);
}

#[tokio::test]
async fn test_show_tweet_with_image_media() {
    use std::process::Command;
    use tempfile::TempDir;

    // Create a temporary directory for this test
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    // Use the real tweet with image media (douglaz tweet with image)
    let tweet_with_image = serde_json::json!({
        "id": "1947427270152626319",
        "text": "@allanraicher https://t.co/RJpDQjcHuj",
        "author": {
            "id": "3916631",
            "name": "douglaz",
            "username": "douglaz",
            "profile_image_url": "https://pbs.twimg.com/profile_images/1639377321386823680/UVcJ5dbZ_normal.jpg"
        },
        "referenced_tweets": [
            {
                "id": "1947423415218147567",
                "type": "replied_to"
            }
        ],
        "attachments": {
            "media_keys": ["3_1947427262086733824"]
        },
        "created_at": "2025-07-21T22:43:37.000Z",
        "entities": {
            "urls": [
                {
                    "url": "https://t.co/RJpDQjcHuj",
                    "expanded_url": "https://x.com/douglaz/status/1947427270152626319/photo/1",
                    "display_url": "pic.x.com/RJpDQjcHuj"
                }
            ],
            "mentions": [
                {
                    "username": "allanraicher"
                }
            ]
        },
        "includes": {
            "media": [
                {
                    "media_key": "3_1947427262086733824",
                    "type": "photo",
                    "url": "https://pbs.twimg.com/media/GwamxuaXYAARXmF.jpg"
                }
            ]
        },
        "author_id": "3916631"
    });

    let tweet_path = temp_dir
        .path()
        .join("20250721_224337_douglaz_1947427270152626319.json");
    let tweet_json =
        serde_json::to_string_pretty(&tweet_with_image).expect("Failed to serialize tweet");
    std::fs::write(&tweet_path, tweet_json).expect("Failed to write test tweet");

    // Run the show-tweet command
    let output = Command::new("cargo")
        .args([
            "run",
            "--",
            "show-tweet",
            "1947427270152626319",
            "--data-dir",
            temp_dir.path().to_str().unwrap(),
            "--mnemonic",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ])
        .output()
        .expect("Failed to execute command");

    // Check that the command succeeded
    assert!(
        output.status.success(),
        "Command failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout_str = String::from_utf8(output.stdout).expect("Invalid UTF-8 in stdout");
    let json: serde_json::Value =
        serde_json::from_str(&stdout_str).expect("stdout should be valid JSON");

    // Verify the Nostr event content includes the actual image URL
    let nostr_content = json["nostr"]["event"]["content"].as_str().unwrap();

    // Should contain the actual Twitter media URL inline where the t.co URL was
    assert!(
        nostr_content.contains("@allanraicher https://pbs.twimg.com/media/GwamxuaXYAARXmF.jpg"),
        "Nostr content should contain the actual image URL inline: {nostr_content}"
    );

    // Should NOT contain the t.co URL anymore since it was replaced
    assert!(
        !nostr_content.contains("https://t.co/RJpDQjcHuj"),
        "Nostr content should not contain the t.co URL since it was replaced: {nostr_content}"
    );

    // Should NOT contain the Twitter page link since it was replaced with the direct image URL
    assert!(
        !nostr_content.contains("https://x.com/douglaz/status/1947427270152626319/photo/1"),
        "Nostr content should not contain the Twitter page link: {nostr_content}"
    );

    // The image URL should appear exactly once (inline replacement, no duplication)
    let image_url_count = nostr_content
        .matches("https://pbs.twimg.com/media/GwamxuaXYAARXmF.jpg")
        .count();
    pretty_assertions::assert_eq!(
        image_url_count,
        1,
        "Nostr content should contain exactly one instance of the image URL (no duplication): {nostr_content}"
    );

    // Verify the original Twitter data is preserved
    let twitter_data = &json["twitter"];
    pretty_assertions::assert_eq!(twitter_data["id"], "1947427270152626319");
    pretty_assertions::assert_eq!(twitter_data["author"]["username"], "douglaz");

    // Verify the media is present in the Twitter data
    let includes_media = &twitter_data["includes"]["media"];
    assert!(includes_media.is_array());
    let media_array = includes_media.as_array().unwrap();
    pretty_assertions::assert_eq!(media_array.len(), 1);
    pretty_assertions::assert_eq!(
        media_array[0]["url"],
        "https://pbs.twimg.com/media/GwamxuaXYAARXmF.jpg"
    );
    pretty_assertions::assert_eq!(media_array[0]["type"], "photo");
}

fn create_tweet_with_referenced_tweet_media() -> serde_json::Value {
    serde_json::json!({
        "id": "1946563939120169182",
        "text": "@SandLabs_21 Por enquanto estou sozinho aqui em Asunción",
        "author": {
            "id": "3916631",
            "name": "douglaz",
            "username": "douglaz",
            "profile_image_url": "https://pbs.twimg.com/profile_images/1639377321386823680/UVcJ5dbZ_normal.jpg"
        },
        "author_id": "3916631",
        "created_at": "2025-01-03T01:54:55.000Z",
        "entities": {
            "mentions": [
                {"username": "SandLabs_21"}
            ]
        },
        "referenced_tweets": [
            {
                "id": "1946508933071319212",
                "type": "replied_to",
                "data": {
                    "id": "1946508933071319212",
                    "text": "Aparentemente a rede lora funciona muito bem \nE pra funcionar bem no Brasil só depende de ter mais malucos querendo conversar de forma privada sem precisar da internet https://t.co/cNeKMqGbe6",
                    "author": {
                        "id": "1234567890",
                        "name": "SandLabs_21",
                        "username": "SandLabs_21",
                        "profile_image_url": "https://pbs.twimg.com/profile_images/example.jpg"
                    },
                    "author_id": "1234567890",
                    "created_at": "2025-01-03T01:15:32.000Z",
                    "entities": {
                        "urls": [
                            {
                                "url": "https://t.co/cNeKMqGbe6",
                                "expanded_url": "https://x.com/SandLabs_21/status/1946508933071319212/video/1",
                                "display_url": "pic.x.com/cNeKMqGbe6"
                            }
                        ]
                    },
                    "includes": {
                        "media": [
                            {
                                "media_key": "7_1946508858128416768",
                                "type": "video",
                                "url": null,
                                "preview_image_url": "https://pbs.twimg.com/ext_tw_video_thumb/1946508858128416768/pu/img/example.jpg",
                                "variants": [
                                    {
                                        "bit_rate": 288000,
                                        "content_type": "video/mp4",
                                        "url": "https://video.twimg.com/ext_tw_video/1946508750375759873/pu/vid/avc1/480x852/wdUR-OhqpJ8CQyTq.mp4?tag=12"
                                    },
                                    {
                                        "bit_rate": 832000,
                                        "content_type": "video/mp4",
                                        "url": "https://video.twimg.com/ext_tw_video/1946508750375759873/pu/vid/avc1/720x1280/wdUR-OhqpJ8CQyTq.mp4?tag=12"
                                    }
                                ]
                            }
                        ]
                    }
                }
            }
        ]
    })
}

#[tokio::test]
async fn test_show_tweet_with_referenced_tweet_media() {
    use std::process::Command;
    use tempfile::TempDir;

    // Create a test tweet with referenced tweet that has media - tweet 1946563939120169182
    let tweet_with_ref_media = create_tweet_with_referenced_tweet_media();

    // Create temporary directory for test
    let temp_dir = TempDir::new().unwrap();

    // Write test tweet to file
    let tweet_path = temp_dir
        .path()
        .join("20250103_015455_douglaz_1946563939120169182.json");
    let tweet_json =
        serde_json::to_string_pretty(&tweet_with_ref_media).expect("Failed to serialize tweet");
    std::fs::write(&tweet_path, tweet_json).expect("Failed to write test tweet");

    // Run the show-tweet command
    let output = Command::new("cargo")
        .args([
            "run",
            "--",
            "show-tweet",
            "1946563939120169182",
            "--data-dir",
            temp_dir.path().to_str().unwrap(),
            "--mnemonic",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ])
        .output()
        .expect("Failed to execute command");

    // Check that the command succeeded
    assert!(
        output.status.success(),
        "Command failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Parse the JSON output from stdout
    let stdout = String::from_utf8(output.stdout).expect("Invalid UTF-8 in stdout");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("Invalid JSON in stdout");

    // Extract the Nostr content
    let nostr_content = json["nostr"]["event"]["content"]
        .as_str()
        .expect("Nostr content should be a string");

    // Check that the referenced tweet content includes proper quoted text and media URL expansion
    // Now uses Nostr mention resolution instead of Twitter usernames
    assert!(
        nostr_content.contains("↩️ Reply to nostr:npub")
            || nostr_content.contains("↩️ Reply to @SandLabs_21:"),
        "Should show proper reply formatting with Nostr mentions: {nostr_content}"
    );

    // Should contain referenced tweet content (without quotes)
    assert!(
        nostr_content.contains("Aparentemente a rede lora funciona muito bem"),
        "Should contain referenced tweet content: {nostr_content}"
    );

    // Should contain actual media URL instead of t.co link in referenced tweet
    assert!(
        nostr_content.contains("https://video.twimg.com/ext_tw_video/1946508750375759873/pu/vid/avc1/720x1280/wdUR-OhqpJ8CQyTq.mp4?tag=12"),
        "Should contain actual video URL in referenced tweet: {nostr_content}"
    );

    // Should NOT contain t.co URL in the referenced tweet content
    assert!(
        !nostr_content.contains("https://t.co/cNeKMqGbe6"),
        "Should not contain t.co URL in referenced tweet: {nostr_content}"
    );

    // Since the unified formatting now uses Nostr mentions, check key components instead of exact match
    // The exact npub will vary based on key generation, so we check for structural elements
    assert!(
        nostr_content
            .contains("🐦 @douglaz: @SandLabs_21 Por enquanto estou sozinho aqui em Asunción"),
        "Should contain main tweet content"
    );
    assert!(
        nostr_content.contains("↩️ Reply to nostr:npub"),
        "Should contain Nostr mention format"
    );
    assert!(
        nostr_content.contains("Aparentemente a rede lora funciona muito bem"),
        "Should contain referenced tweet content"
    );
    assert!(
        nostr_content.contains("https://video.twimg.com/ext_tw_video/1946508750375759873/pu/vid/avc1/720x1280/wdUR-OhqpJ8CQyTq.mp4?tag=12"),
        "Should contain direct video URL"
    );
    assert!(
        nostr_content.contains("Original tweet: https://twitter.com/i/status/1946563939120169182"),
        "Should contain original tweet link"
    );
}

#[tokio::test]
async fn test_douglaz_retweet_with_video_inline() {
    let tweet = fixtures::douglaz_retweet_with_video();
    let keys = Keys::generate();
    let media_urls = vec![];

    let event = create_nostr_event_from_tweet(&tweet, &media_urls, &keys)
        .await
        .expect("Failed to create Nostr event");

    // Verify event structure
    pretty_assertions::assert_eq!(event.kind, Kind::TextNote);
    pretty_assertions::assert_eq!(event.pubkey, keys.public_key());

    // Verify content formatting
    let content = &event.content;

    // Check that it's formatted as a retweet
    assert!(content.contains("🔁 @douglaz retweeted @newstart_2024:"));

    // Check that the Frank Zappa quote is present
    assert!(content.contains("Frank Zappa, 1981: \"Schools train people to be ignorant"));
    assert!(content.contains("functional ignoramus"));
    assert!(content.contains("military industrial complex"));

    // THE KEY TEST: The video URL should be inline where the t.co URL was
    // The text ends with "do a job," followed by the t.co URL in the original
    // We want to ensure the video URL replaces the t.co URL inline
    assert!(
        content.contains("do a job, https://video.twimg.com/amplify_video/1949544286111809536/vid/avc1/480x352/JFYPmmXfT3EQV8aE.mp4"),
        "Video URL should be inline where the t.co URL was, not separated. Content: {content}"
    );

    // Make sure the t.co URL is NOT present
    assert!(
        !content.contains("https://t.co/JsfJ9Q4ndA"),
        "t.co URL should be replaced, not present. Content: {content}"
    );

    // The video URL should NOT appear again after the Twitter link
    let twitter_link_pos = content
        .find("https://twitter.com/i/status/1949546554764705845")
        .unwrap();
    let video_url = "https://video.twimg.com/amplify_video/1949544286111809536/vid/avc1/480x352/JFYPmmXfT3EQV8aE.mp4";

    // Find all occurrences of the video URL
    let video_occurrences: Vec<_> = content.match_indices(video_url).collect();

    // Should only appear once (inline)
    pretty_assertions::assert_eq!(
        video_occurrences.len(),
        1,
        "Video URL should appear exactly once (inline). Content: {content}"
    );

    // And that one occurrence should be before the Twitter link
    assert!(
        video_occurrences[0].0 < twitter_link_pos,
        "Video URL should appear before the Twitter link, not after. Content: {content}"
    );

    // Verify the original tweet link is present
    assert!(content.contains("Original tweet: https://twitter.com/i/status/1949597796660551797"));
}

#[test]
fn test_extract_media_urls_from_retweeted_tweet() {
    let tweet = fixtures::douglaz_retweet_with_video();

    // Check if the main tweet has media
    let main_media_urls = extract_media_urls_from_tweet(&tweet);
    println!("Main tweet media URLs: {main_media_urls:?}");

    // Check media from referenced tweets
    if let Some(ref_tweets) = &tweet.referenced_tweets {
        for ref_tweet in ref_tweets {
            if let Some(ref_data) = &ref_tweet.data {
                let ref_media_urls = extract_media_urls_from_tweet(ref_data);
                println!(
                    "Referenced tweet {} media URLs: {ref_media_urls:?}",
                    ref_data.id
                );

                // For the retweeted tweet, we expect to find the video URL
                if ref_tweet.type_field == "retweeted" {
                    assert!(
                        !ref_media_urls.is_empty(),
                        "Retweeted tweet should have media URLs"
                    );

                    // Should contain the video URL
                    let has_video = ref_media_urls
                        .iter()
                        .any(|url| url.contains("video.twimg.com"));
                    assert!(
                        has_video,
                        "Should extract video URL from retweeted tweet. Found: {ref_media_urls:?}"
                    );

                    // Check the text
                    println!("Retweeted tweet text: {}", ref_data.text);
                    if let Some(note) = &ref_data.note_tweet {
                        println!("Retweeted tweet note text: {}", note.text);
                    }

                    // Check entities
                    if let Some(entities) = &ref_data.entities {
                        println!("Retweeted tweet entities: {entities:?}");
                    }
                }
            }
        }
    }
}
