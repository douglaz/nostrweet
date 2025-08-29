use crate::error_utils::create_http_client_with_context;
use crate::filename_utils::{media_filename, sanitized_file_path};
use crate::twitter::{Media as TwitterMedia, Tweet};
use anyhow::{Context, Result, ensure};
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use reqwest::Client;
use std::io::Read;
use std::path::{Path, PathBuf};
use tokio::fs::{File, metadata};
use tokio::io::AsyncWriteExt;
use tokio_util::io::StreamReader;
use tracing::{debug, info, warn};

/// Represents the result of a media download operation
pub struct MediaResult {
    /// Path where the media file is located
    pub file_path: PathBuf,
    /// Whether the file was already in the cache
    pub from_cache: bool,
}

/// Extract all media URLs from a tweet's includes.media section and entities.urls
pub fn extract_media_urls_from_tweet(tweet: &Tweet) -> Vec<String> {
    let mut media_urls = Vec::new();

    // Helper function to process a single media item
    fn process_media_item(media: &TwitterMedia) -> Option<String> {
        // Handle photos (direct URL in url field)
        if let Some(url) = &media.url {
            return Some(url.clone());
        }

        // Handle videos and GIFs (URLs in variants)
        if media.type_field == "video" || media.type_field == "animated_gif" {
            if let Some(variants) = &media.variants {
                // Find the variant with the highest bitrate (highest quality)
                return variants
                    .iter()
                    .filter_map(|v| v.bit_rate.map(|br| (br, &v.url)))
                    .max_by_key(|&(br, _)| br)
                    .map(|(_, url)| url.clone());
            }
        }

        // Fallback to preview image if available
        media.preview_image_url.clone()
    }

    // Extract possible video URLs from entities.urls (needed when Twitter doesn't include media variants)
    fn extract_video_urls_from_entities(
        entities: &Option<crate::twitter::Entities>,
    ) -> Vec<String> {
        let mut video_urls = Vec::new();

        if let Some(entities) = entities {
            if let Some(urls) = &entities.urls {
                for url_entity in urls {
                    let expanded_url = &url_entity.expanded_url;

                    // Check if this is a Twitter/X video URL
                    if expanded_url.contains("video")
                        && (expanded_url.contains("twitter.com") || expanded_url.contains("x.com"))
                    {
                        debug!("Found Twitter video URL in entities: {expanded_url}");
                        video_urls.push(expanded_url.clone());
                    }
                }
            }
        }

        video_urls
    }

    // Extract media from the main tweet
    if let Some(includes) = &tweet.includes {
        if let Some(media_items) = &includes.media {
            for media in media_items {
                if let Some(url) = process_media_item(media) {
                    media_urls.push(url);
                }
            }
        }
    }

    // If we have no media URLs found via includes.media, try to extract from entities.urls
    if media_urls.is_empty() {
        // Try to get video URLs from entities
        let mut entity_video_urls = extract_video_urls_from_entities(&tweet.entities);
        media_urls.append(&mut entity_video_urls);

        // Also check referenced tweets for video URLs in their entities
        if let Some(referenced_tweets) = &tweet.referenced_tweets {
            for ref_tweet in referenced_tweets {
                if let Some(ref_data) = &ref_tweet.data {
                    let mut ref_video_urls = extract_video_urls_from_entities(&ref_data.entities);
                    media_urls.append(&mut ref_video_urls);
                }
            }
        }
    } else {
        // Even if we have some media, check if any entity URLs indicate video content that we might need to fetch
        if let Some(entities) = &tweet.entities {
            if let Some(urls) = &entities.urls {
                for url_entity in urls {
                    let expanded_url = &url_entity.expanded_url;
                    if expanded_url.contains("video")
                        && (expanded_url.contains("twitter.com") || expanded_url.contains("x.com"))
                    {
                        debug!(
                            "Tweet might contain video content not fully expanded: {expanded_url}"
                        );
                    }
                }
            }
        }
    }

    // Remove duplicates
    media_urls.dedup();

    media_urls
}

/// Downloads all media from a tweet to the specified directory
pub async fn download_media(
    tweet: &Tweet,
    data_dir: &Path,
    bearer_token: Option<&str>,
) -> Result<Vec<MediaResult>> {
    let mut media_files = Vec::new();
    let client = create_http_client_with_context()?;

    // Process media from the main tweet
    if let Some(includes) = &tweet.includes {
        if let Some(media_items) = &includes.media {
            for media in media_items.iter() {
                match download_media_item(&client, media, tweet, data_dir, bearer_token).await {
                    Ok(result) => media_files.push(result),
                    Err(e) => {
                        warn!(
                            "Failed to download media item (type: {media_type}, key: {media_key}) for tweet {tweet_id}: {error}",
                            media_type = media.type_field,
                            media_key = media.media_key,
                            tweet_id = tweet.id,
                            error = e
                        );
                        // Optionally, attempt to clean up partially created file if necessary,
                        // but for now, just logging and continuing is the main goal.
                    }
                }
            }
        }
    }

    // Process media from all referenced tweets (retweets, quotes, replies)
    if let Some(referenced_tweets) = &tweet.referenced_tweets {
        for ref_tweet in referenced_tweets {
            // Process any referenced tweet that has data (previously we only processed "retweeted" type)
            if let Some(original_tweet) = &ref_tweet.data {
                let ref_type = &ref_tweet.type_field;
                debug!(
                    "Processing media from {ref_type} tweet {}",
                    original_tweet.id
                );

                if let Some(includes) = &original_tweet.includes {
                    if let Some(media_items) = &includes.media {
                        for media_item in media_items.iter() {
                            match download_media_item(
                                &client,
                                media_item,
                                original_tweet,
                                data_dir,
                                bearer_token,
                            )
                            .await
                            {
                                Ok(result) => media_files.push(result),
                                Err(e) => {
                                    warn!(
                                        "Failed to download media item from referenced tweet (original_tweet_id: {id}, media_type: {field_type}, media_key: {key}): {e}",
                                        id = original_tweet.id,
                                        field_type = media_item.type_field,
                                        key = media_item.media_key
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Count and log the files by source (cache vs downloaded)
    let cached_count = media_files
        .iter()
        .filter(|result| result.from_cache)
        .count();
    let downloaded_count = media_files.len() - cached_count;

    if downloaded_count > 0 {
        info!("Downloaded {downloaded_count} new media files");
    }

    if cached_count > 0 {
        info!("Found {cached_count} media files already in cache");
    }

    info!("Total media files: {total}", total = media_files.len());

    Ok(media_files)
}

/// Determine the appropriate download URL for a media item
fn determine_download_url(media: &TwitterMedia) -> Result<&String> {
    match media.type_field.as_str() {
        "photo" => {
            // For photos, use the URL directly
            media.url.as_ref().context("Photo URL missing")
        }
        "video" | "animated_gif" => {
            // For videos, find the highest quality variant
            let variants = media.variants.as_ref().context("Video variants missing")?;
            ensure!(!variants.is_empty(), "No video variants found");

            // Find the highest bitrate variant
            let best_variant = variants
                .iter()
                .filter(|v| v.url.contains(".mp4"))
                .max_by_key(|v| v.bit_rate.unwrap_or(0))
                .or_else(|| variants.first())
                .context("No suitable video variant found")?;

            Ok(&best_variant.url)
        }
        other => {
            debug!("Unknown media type: {other}, using preview image");
            // Fall back to preview image if available
            media
                .preview_image_url
                .as_ref()
                .context("No preview image URL for unknown media type")
        }
    }
}

/// Get the appropriate file extension for a media type
fn get_file_extension(media_type: &str) -> &'static str {
    match media_type {
        "photo" => "jpg",
        "video" | "animated_gif" => "mp4",
        _ => "jpg",
    }
}

/// Check if media file exists in cache and return result if valid
async fn check_media_cache(file_path: &PathBuf) -> Result<Option<MediaResult>> {
    if let Ok(metadata) = metadata(file_path).await {
        // File exists, check if it's non-empty (has content)
        if metadata.len() > 0 {
            debug!(
                "Found cached media file: {path}",
                path = file_path.display()
            );
            return Ok(Some(MediaResult {
                file_path: file_path.clone(),
                from_cache: true,
            }));
        } else {
            // File exists but is empty, we'll redownload it
            warn!(
                "Found empty file, will redownload: {path}",
                path = file_path.display()
            );
        }
    }
    Ok(None)
}

/// Build a properly configured HTTP request for media download
fn build_media_request(
    client: &Client,
    download_url: &str,
    media_type: &str,
    bearer_token: Option<&str>,
) -> Result<reqwest::RequestBuilder> {
    // Build request with headers to avoid 403 on protected media
    let mut req_builder = client
        .get(download_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64; rv:139.0) Gecko/20100101 Firefox/139.0",
        )
        .header("Accept-Language", "en-US,en;q=0.5")
        .header("Accept-Encoding", "gzip, deflate, br, zstd")
        .header("DNT", "1")
        .header("Sec-GPC", "1")
        .header("Connection", "keep-alive");

    // Set headers based on media type
    req_builder = match media_type {
        "photo" => req_builder
            .header(
                "Accept",
                "image/jpeg,image/png,image/webp,image/avif,image/*;q=0.8,*/*;q=0.5",
            )
            .header("Sec-Fetch-Dest", "image")
            .header("Sec-Fetch-Mode", "no-cors")
            .header("Sec-Fetch-Site", "cross-site")
            .header("Referer", "https://x.com/")
            .header("Origin", "https://x.com"),
        "video" | "animated_gif" => req_builder
            .header("Accept", "video/mp4,video/webm,video/*;q=0.8,*/*;q=0.5")
            .header("Sec-Fetch-Dest", "video")
            .header("Sec-Fetch-Mode", "no-cors")
            .header("Sec-Fetch-Site", "cross-site")
            .header("Referer", "https://x.com/")
            .header("Origin", "https://x.com"),
        _ => req_builder
            .header(
                "Accept",
                "image/jpeg,image/png,image/webp,image/avif,image/*;q=0.8,*/*;q=0.5",
            )
            .header("Sec-Fetch-Dest", "image")
            .header("Sec-Fetch-Mode", "no-cors")
            .header("Sec-Fetch-Site", "cross-site")
            .header("Referer", "https://x.com/")
            .header("Origin", "https://x.com"),
    };

    // Conditionally attach bearer token for non-public hosts
    if !(download_url.contains("pbs.twimg.com") || download_url.contains("video.twimg.com")) {
        if let Some(bearer) = bearer_token {
            req_builder = req_builder.bearer_auth(bearer);
            debug!("Using bearer token for URL: {download_url}");
        } else {
            warn!("Bearer token not provided. Media download for URL ({download_url}) may fail.",);
        }
    } else {
        debug!("Skipping bearer token for known public media URL: {download_url}");
    }

    Ok(req_builder)
}

/// Handle download error responses with detailed logging and cleanup
async fn handle_download_error(
    response: reqwest::Response,
    download_url: &str,
    file_path: &PathBuf,
    media_key: &str,
    tweet_id: &str,
) -> Result<()> {
    let status = response.status();
    let response_headers = response.headers().clone();
    let response_bytes_result = response.bytes().await;

    let body_text = match response_bytes_result {
        Ok(bytes) => {
            let mut decoded_bytes = Vec::new();
            let is_gzipped = response_headers
                .get(reqwest::header::CONTENT_ENCODING)
                .is_some_and(|h| h == "gzip");

            if is_gzipped {
                let mut decoder = GzDecoder::new(&bytes[..]);
                if decoder.read_to_end(&mut decoded_bytes).is_ok() {
                    String::from_utf8(decoded_bytes)
                        .unwrap_or_else(|e| format!("Non-UTF8 gzipped body: {e}"))
                } else {
                    // Fallback if gzip decoding fails
                    String::from_utf8(bytes.to_vec()).unwrap_or_else(|e| {
                        format!(
                            "Non-UTF8 binary body ({bytes_len} bytes, gzip decoding failed): {e}",
                            bytes_len = bytes.len()
                        )
                    })
                }
            } else {
                // Not gzipped, try UTF-8 directly
                String::from_utf8(bytes.to_vec()).unwrap_or_else(|e| {
                    format!(
                        "Non-UTF8 binary body ({bytes_len} bytes): {e}",
                        bytes_len = bytes.len()
                    )
                })
            }
        }
        Err(e) => format!("Could not read response body: {e}"),
    };

    warn!(
        "Failed media download. Status: {status}. URL: {download_url}. Headers: {response_headers:#?}. Body: {body_text}"
    );

    // Attempt to remove the partially created file if download failed
    if let Err(e) = tokio::fs::remove_file(file_path).await {
        warn!(
            "Failed to remove partially downloaded file {path}: {e}",
            path = file_path.display()
        );
    }

    Err(anyhow::anyhow!(
        "Failed to download media {media_key} for tweet {tweet_id} from {download_url}: HTTP status {status}",
    ))
}

/// Stream HTTP response to file with progress logging
async fn stream_response_to_file(response: reqwest::Response, file: &mut File) -> Result<()> {
    let total_size = response.content_length().unwrap_or(0);

    // Stream the response to file
    let stream = response.bytes_stream();
    let mut reader = StreamReader::new(stream.map(|result| result.map_err(std::io::Error::other)));

    let mut buffer = vec![0u8; 8192];
    let mut downloaded = 0;

    use tokio::io::AsyncReadExt;
    while let Ok(n) = reader.read(&mut buffer).await {
        if n == 0 {
            break;
        }

        file.write_all(&buffer[..n])
            .await
            .context("Failed to write media data to file")?;

        downloaded += n as u64;

        if total_size > 0 {
            let progress = (downloaded * 100) / total_size;
            debug!("Download progress: {progress}%");
        }
    }

    file.flush().await.context("Failed to flush file")?;
    Ok(())
}

/// Downloads a single media item and returns the result with file path and cache status
async fn download_media_item(
    client: &Client,
    media: &TwitterMedia,
    tweet: &Tweet,
    data_dir: &Path,
    bearer_token: Option<&str>,
) -> Result<MediaResult> {
    let download_url = determine_download_url(media)?;
    let file_extension = get_file_extension(&media.type_field);

    // Extract the media_key
    let media_key = &media.media_key;

    // Include tweet author in the filename for better organization, but use media_key as the main identifier
    let filename = media_filename(&tweet.author.username, media_key, file_extension);
    let file_path = sanitized_file_path(data_dir, &filename);

    // Check if we already have this file cached
    if let Some(cached_result) = check_media_cache(&file_path).await? {
        return Ok(cached_result);
    }

    // Create the output file
    let mut file = File::create(&file_path)
        .await
        .context("Failed to create output file")?;

    // Download the media file
    info!(
        "Downloading {type_field} from {download_url}",
        type_field = media.type_field
    );

    let req_builder = build_media_request(client, download_url, &media.type_field, bearer_token)?;

    // Debug: log the prepared request headers to diagnose 403
    if let Some(rb) = req_builder.try_clone() {
        if let Ok(request) = rb.build() {
            debug!(
                "Media download request: {} {}",
                request.method(),
                request.url()
            );
            for (name, value) in request.headers() {
                debug!("Header {name}: {value:?}");
            }
        }
    }

    let response = req_builder
        .send()
        .await
        .context("Failed to download media")?;

    if !response.status().is_success() {
        handle_download_error(
            response,
            download_url,
            &file_path,
            &media.media_key,
            &tweet.id,
        )
        .await?;
        // Return early since handle_download_error will always return an error
        unreachable!("handle_download_error should always return an error");
    }

    stream_response_to_file(response, &mut file).await?;

    debug!("Saved new media to {path}", path = file_path.display());

    Ok(MediaResult {
        file_path,
        from_cache: false,
    })
}
