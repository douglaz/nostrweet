use serde::{Deserialize, Serialize};

use crate::ids::{HttpUrl, MediaKey};
use crate::twitter::{Entities, Tweet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Photo,
    Video,
    AnimatedGif,
    Other(String),
}

impl Serialize for MediaKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let value = match self {
            MediaKind::Photo => "photo",
            MediaKind::Video => "video",
            MediaKind::AnimatedGif => "animated_gif",
            MediaKind::Other(value) => value.as_str(),
        };
        serializer.serialize_str(value)
    }
}

impl<'de> Deserialize<'de> for MediaKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "photo" => MediaKind::Photo,
            "video" => MediaKind::Video,
            "animated_gif" => MediaKind::AnimatedGif,
            other => MediaKind::Other(other.to_string()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    pub media_key: MediaKey,
    #[serde(rename = "type")]
    pub kind: MediaKind,
    pub url: Option<HttpUrl>,
    pub preview_image_url: Option<HttpUrl>,
    pub alt_text: Option<String>,
    #[serde(default)]
    pub variants: Vec<MediaVariant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaVariant {
    pub bit_rate: Option<u64>,
    pub content_type: String,
    pub url: HttpUrl,
}

#[derive(Clone, Debug)]
pub struct RawTweet {
    pub tweet: Tweet,
}

#[derive(Clone, Debug)]
pub struct EnrichedTweet {
    pub tweet: Tweet,
    pub media_urls: Vec<HttpUrl>,
}

impl From<Tweet> for RawTweet {
    fn from(tweet: Tweet) -> Self {
        Self { tweet }
    }
}

impl From<RawTweet> for EnrichedTweet {
    fn from(raw: RawTweet) -> Self {
        let media_urls = extract_media_urls(&raw.tweet);
        Self {
            tweet: raw.tweet,
            media_urls,
        }
    }
}

pub fn extract_media_urls(tweet: &Tweet) -> Vec<HttpUrl> {
    let mut media_urls = Vec::new();

    fn process_media_item(media: &Media) -> Option<HttpUrl> {
        if let Some(url) = &media.url {
            return Some(url.clone());
        }

        match media.kind {
            MediaKind::Video | MediaKind::AnimatedGif => {
                let best_variant = media
                    .variants
                    .iter()
                    .filter_map(|variant| variant.bit_rate.map(|br| (br, &variant.url)))
                    .max_by_key(|&(br, _)| br)
                    .map(|(_, url)| url.clone());

                if best_variant.is_some() {
                    return best_variant;
                }
            }
            _ => {}
        }

        media.preview_image_url.clone()
    }

    fn extract_video_urls_from_entities(entities: &Option<Entities>) -> Vec<HttpUrl> {
        let mut video_urls = Vec::new();

        if let Some(entities) = entities {
            for url_entity in &entities.urls {
                let expanded_url = url_entity.expanded_url.as_ref().unwrap_or(&url_entity.url);
                let expanded_str = expanded_url.as_str();
                if expanded_str.contains("video")
                    && (expanded_str.contains("twitter.com") || expanded_str.contains("x.com"))
                {
                    video_urls.push(expanded_url.clone());
                }
            }
        }

        video_urls
    }

    if let Some(includes) = &tweet.includes {
        for media in &includes.media {
            if let Some(url) = process_media_item(media) {
                media_urls.push(url);
            }
        }
    }

    if media_urls.is_empty() {
        media_urls.extend(extract_video_urls_from_entities(&tweet.entities));

        for referenced in &tweet.referenced_tweets {
            if let Some(ref_data) = &referenced.data {
                media_urls.extend(extract_video_urls_from_entities(&ref_data.entities));
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    media_urls.retain(|url| seen.insert(url.as_str().to_string()));

    media_urls
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CreatedAt, TweetId, UserId, Username};
    use crate::twitter::{TweetText, User};

    #[test]
    fn extract_media_urls_prefers_best_variant() -> anyhow::Result<()> {
        let media = Media {
            media_key: MediaKey::parse("3_123")?,
            kind: MediaKind::Video,
            url: None,
            preview_image_url: None,
            alt_text: None,
            variants: vec![
                MediaVariant {
                    bit_rate: Some(100),
                    content_type: "video/mp4".to_string(),
                    url: HttpUrl::parse("https://video.example.com/low.mp4")?,
                },
                MediaVariant {
                    bit_rate: Some(200),
                    content_type: "video/mp4".to_string(),
                    url: HttpUrl::parse("https://video.example.com/high.mp4")?,
                },
            ],
        };

        let tweet = Tweet::new(
            TweetId::parse("123")?,
            TweetText::parse("hello")?,
            User::new(UserId::parse("1")?, Username::parse("tester")?),
            CreatedAt::parse("2023-01-01T00:00:00Z")?,
        );

        let mut tweet = tweet;
        tweet.includes = Some(crate::twitter::Includes {
            media: vec![media],
            users: Vec::new(),
            tweets: Vec::new(),
        });

        let urls = extract_media_urls(&tweet);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].as_str(), "https://video.example.com/high.mp4");
        Ok(())
    }
}
