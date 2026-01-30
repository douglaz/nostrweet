use serde::{Deserialize, Serialize};

use crate::ids::{CreatedAt, HttpUrl, MediaKey, TweetId, UserId, Username};
use crate::media::Media;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TweetText(String);

impl TweetText {
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        let trimmed = input.trim();
        anyhow::ensure!(!trimmed.is_empty(), "Tweet text cannot be empty");
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TweetText {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        TweetText::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<TweetText> for String {
    fn from(value: TweetText) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteTweet {
    pub text: TweetText,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub name: Option<String>,
    pub username: Username,
    pub profile_image_url: Option<HttpUrl>,
    pub description: Option<String>,
    pub url: Option<HttpUrl>,
    pub entities: Option<UserEntities>,
}

impl User {
    pub fn new(id: UserId, username: Username) -> Self {
        Self {
            id,
            name: None,
            username,
            profile_image_url: None,
            description: None,
            url: None,
            entities: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserEntities {
    pub url: Option<UserUrlEntity>,
    pub description: Option<Entities>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserUrlEntity {
    #[serde(default)]
    pub urls: Vec<UrlEntity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tweet {
    pub id: TweetId,
    pub text: TweetText,
    pub author: User,
    #[serde(default)]
    pub referenced_tweets: Vec<ReferencedTweet>,
    pub attachments: Option<Attachments>,
    pub created_at: CreatedAt,
    pub entities: Option<Entities>,
    pub includes: Option<Includes>,
    pub author_id: Option<UserId>,
    pub note_tweet: Option<NoteTweet>,
}

impl Tweet {
    pub fn new(id: TweetId, text: TweetText, author: User, created_at: CreatedAt) -> Self {
        Self {
            id,
            text,
            author,
            referenced_tweets: Vec::new(),
            attachments: None,
            created_at,
            entities: None,
            includes: None,
            author_id: None,
            note_tweet: None,
        }
    }

    pub fn builder() -> TweetDraft<Missing, Missing, Missing, Missing> {
        TweetDraft::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferencedTweet {
    pub id: TweetId,
    #[serde(rename = "type")]
    pub kind: ReferenceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<Tweet>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    RepliedTo,
    Retweeted,
    Quoted,
    Unknown(String),
}

impl Serialize for ReferenceKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let value = match self {
            ReferenceKind::RepliedTo => "replied_to",
            ReferenceKind::Retweeted => "retweeted",
            ReferenceKind::Quoted => "quoted",
            ReferenceKind::Unknown(value) => value.as_str(),
        };
        serializer.serialize_str(value)
    }
}

impl<'de> Deserialize<'de> for ReferenceKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "replied_to" => ReferenceKind::RepliedTo,
            "retweeted" => ReferenceKind::Retweeted,
            "quoted" => ReferenceKind::Quoted,
            other => ReferenceKind::Unknown(other.to_string()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachments {
    #[serde(default)]
    pub media_keys: Vec<MediaKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entities {
    #[serde(default)]
    pub urls: Vec<UrlEntity>,
    #[serde(default)]
    pub mentions: Vec<Mention>,
    #[serde(default)]
    pub hashtags: Vec<Hashtag>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlEntity {
    pub url: HttpUrl,
    pub expanded_url: Option<HttpUrl>,
    pub display_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mention {
    pub username: Username,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hashtag {
    pub tag: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Includes {
    #[serde(default)]
    pub media: Vec<Media>,
    #[serde(default)]
    pub users: Vec<User>,
    #[serde(default)]
    pub tweets: Vec<Tweet>,
}

pub struct Missing;

pub struct Present<T>(pub T);

pub struct TweetDraft<Id, Text, Author, CreatedAt> {
    id: Id,
    text: Text,
    author: Author,
    created_at: CreatedAt,
    referenced_tweets: Vec<ReferencedTweet>,
    attachments: Option<Attachments>,
    entities: Option<Entities>,
    includes: Option<Includes>,
    author_id: Option<UserId>,
    note_tweet: Option<NoteTweet>,
}

impl TweetDraft<Missing, Missing, Missing, Missing> {
    fn new() -> Self {
        Self {
            id: Missing,
            text: Missing,
            author: Missing,
            created_at: Missing,
            referenced_tweets: Vec::new(),
            attachments: None,
            entities: None,
            includes: None,
            author_id: None,
            note_tweet: None,
        }
    }
}

impl<Id, Text, Author, CreatedAt> TweetDraft<Id, Text, Author, CreatedAt> {
    pub fn referenced_tweets(mut self, referenced_tweets: Vec<ReferencedTweet>) -> Self {
        self.referenced_tweets = referenced_tweets;
        self
    }

    pub fn attachments(mut self, attachments: Attachments) -> Self {
        self.attachments = Some(attachments);
        self
    }

    pub fn entities(mut self, entities: Entities) -> Self {
        self.entities = Some(entities);
        self
    }

    pub fn includes(mut self, includes: Includes) -> Self {
        self.includes = Some(includes);
        self
    }

    pub fn author_id(mut self, author_id: UserId) -> Self {
        self.author_id = Some(author_id);
        self
    }

    pub fn note_tweet(mut self, note_tweet: NoteTweet) -> Self {
        self.note_tweet = Some(note_tweet);
        self
    }
}

impl<Text, Author, CreatedAt> TweetDraft<Missing, Text, Author, CreatedAt> {
    pub fn id(self, id: TweetId) -> TweetDraft<Present<TweetId>, Text, Author, CreatedAt> {
        TweetDraft {
            id: Present(id),
            text: self.text,
            author: self.author,
            created_at: self.created_at,
            referenced_tweets: self.referenced_tweets,
            attachments: self.attachments,
            entities: self.entities,
            includes: self.includes,
            author_id: self.author_id,
            note_tweet: self.note_tweet,
        }
    }
}

impl<Id, Author, CreatedAt> TweetDraft<Id, Missing, Author, CreatedAt> {
    pub fn text(self, text: TweetText) -> TweetDraft<Id, Present<TweetText>, Author, CreatedAt> {
        TweetDraft {
            id: self.id,
            text: Present(text),
            author: self.author,
            created_at: self.created_at,
            referenced_tweets: self.referenced_tweets,
            attachments: self.attachments,
            entities: self.entities,
            includes: self.includes,
            author_id: self.author_id,
            note_tweet: self.note_tweet,
        }
    }
}

impl<Id, Text, CreatedAt> TweetDraft<Id, Text, Missing, CreatedAt> {
    pub fn author(self, author: User) -> TweetDraft<Id, Text, Present<User>, CreatedAt> {
        TweetDraft {
            id: self.id,
            text: self.text,
            author: Present(author),
            created_at: self.created_at,
            referenced_tweets: self.referenced_tweets,
            attachments: self.attachments,
            entities: self.entities,
            includes: self.includes,
            author_id: self.author_id,
            note_tweet: self.note_tweet,
        }
    }
}

impl<Id, Text, Author> TweetDraft<Id, Text, Author, Missing> {
    pub fn created_at(
        self,
        created_at: CreatedAt,
    ) -> TweetDraft<Id, Text, Author, Present<CreatedAt>> {
        TweetDraft {
            id: self.id,
            text: self.text,
            author: self.author,
            created_at: Present(created_at),
            referenced_tweets: self.referenced_tweets,
            attachments: self.attachments,
            entities: self.entities,
            includes: self.includes,
            author_id: self.author_id,
            note_tweet: self.note_tweet,
        }
    }
}

impl TweetDraft<Present<TweetId>, Present<TweetText>, Present<User>, Present<CreatedAt>> {
    pub fn build(self) -> Tweet {
        Tweet {
            id: self.id.0,
            text: self.text.0,
            author: self.author.0,
            referenced_tweets: self.referenced_tweets,
            attachments: self.attachments,
            created_at: self.created_at.0,
            entities: self.entities,
            includes: self.includes,
            author_id: self.author_id,
            note_tweet: self.note_tweet,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::CreatedAt;

    #[test]
    fn tweet_builder_requires_fields() -> anyhow::Result<()> {
        let tweet = Tweet::builder()
            .id(TweetId::parse("123")?)
            .text(TweetText::parse("hello")?)
            .author(User::new(UserId::parse("456")?, Username::parse("tester")?))
            .created_at(CreatedAt::parse("2023-01-01T00:00:00Z")?)
            .build();

        assert_eq!(tweet.id.as_str(), "123");
        assert_eq!(tweet.author.username.as_str(), "tester");
        Ok(())
    }
}
