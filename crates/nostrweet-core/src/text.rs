use crate::ids::HttpUrl;
use crate::twitter::{Entities, Tweet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandedText {
    pub text: String,
    pub used_media_urls: Vec<HttpUrl>,
}

pub fn decode_html_entities(text: &str) -> String {
    html_escape::decode_html_entities(text).to_string()
}

pub fn expand_urls_in_text(
    text: &str,
    entities: Option<&Entities>,
    media_urls: &[HttpUrl],
    tweet: &Tweet,
) -> ExpandedText {
    let mut result = text.to_string();
    let mut used_media_urls = Vec::new();

    if let Some(entities) = entities {
        let mut sorted_urls: Vec<_> = entities.urls.iter().collect();
        sorted_urls.sort_by_key(|entity| std::cmp::Reverse(entity.url.as_str().len()));

        for url_entity in sorted_urls {
            let expanded_url = url_entity.expanded_url.as_ref().unwrap_or(&url_entity.url);

            if url_entity.url.as_str() != expanded_url.as_str() {
                let is_media_url =
                    is_twitter_media_url(expanded_url.as_str(), &url_entity.display_url);

                if is_media_url {
                    if let Some(media_url) =
                        find_media_url_for_shortened_url(url_entity.url.as_str(), tweet, media_urls)
                    {
                        result =
                            safe_replace_url(&result, url_entity.url.as_str(), media_url.as_str());
                        used_media_urls.push(media_url);
                    } else {
                        let fallback_link = format!(
                            "[{}]({})",
                            sanitize_display_url(&url_entity.display_url),
                            expanded_url.as_str()
                        );
                        result = safe_replace_url(&result, url_entity.url.as_str(), &fallback_link);
                    }
                } else if is_valid_url(expanded_url.as_str()) {
                    let markdown_link = format!(
                        "[{}]({})",
                        sanitize_display_url(&url_entity.display_url),
                        expanded_url.as_str()
                    );
                    result = safe_replace_url(&result, url_entity.url.as_str(), &markdown_link);
                }
            }
        }
    }

    ExpandedText {
        text: result,
        used_media_urls,
    }
}

fn find_media_url_for_shortened_url(
    shortened_url: &str,
    tweet: &Tweet,
    media_urls: &[HttpUrl],
) -> Option<HttpUrl> {
    if media_urls.is_empty() || !shortened_url.contains("t.co") {
        return None;
    }

    if let Some(entities) = &tweet.entities {
        for url_entity in &entities.urls {
            if url_entity.url.as_str() == shortened_url {
                let expanded_url = url_entity.expanded_url.as_ref().unwrap_or(&url_entity.url);
                if is_twitter_media_url(expanded_url.as_str(), &url_entity.display_url) {
                    if expanded_url.as_str().contains("/video/") {
                        return media_urls.last().cloned();
                    }
                    return media_urls.first().cloned();
                }
            }
        }
    }

    if media_urls.len() == 1 {
        return Some(media_urls[0].clone());
    }

    None
}

fn is_twitter_media_url(expanded_url: &str, display_url: &str) -> bool {
    expanded_url.contains("/photo/")
        || expanded_url.contains("/video/")
        || expanded_url.contains("/status/") && display_url.starts_with("pic.")
        || display_url.starts_with("pic.twitter.com")
        || display_url.starts_with("video.twimg.com")
}

fn safe_replace_url(text: &str, old_url: &str, new_url: &str) -> String {
    if text.contains(old_url) {
        text.replace(old_url, new_url)
    } else {
        text.to_string()
    }
}

fn sanitize_display_url(display_url: &str) -> String {
    display_url
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn is_valid_url(url: &str) -> bool {
    url::Url::parse(url).is_ok() && (url.starts_with("http://") || url.starts_with("https://"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CreatedAt, TweetId, UserId, Username};
    use crate::twitter::{Entities, Tweet, TweetText, UrlEntity, User};

    #[test]
    fn expand_urls_with_markdown() -> anyhow::Result<()> {
        let entities = Entities {
            urls: vec![UrlEntity {
                url: HttpUrl::parse("https://t.co/abc123")?,
                expanded_url: Some(HttpUrl::parse("https://example.com/article")?),
                display_url: "example.com/article".to_string(),
            }],
            mentions: Vec::new(),
            hashtags: Vec::new(),
        };

        let tweet = Tweet::new(
            TweetId::parse("123")?,
            TweetText::parse("Check https://t.co/abc123")?,
            User::new(UserId::parse("1")?, Username::parse("tester")?),
            CreatedAt::parse("2023-01-01T00:00:00Z")?,
        );

        let expanded = expand_urls_in_text(tweet.text.as_str(), Some(&entities), &[], &tweet);
        assert!(
            expanded
                .text
                .contains("[example.com/article](https://example.com/article)")
        );
        Ok(())
    }
}
