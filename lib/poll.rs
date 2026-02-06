impl Bot {
    pub fn start_poll(&self, discord: serenity::Context, interval: Duration) {
        let this = self.clone();
        tokio::spawn(async move { this.poll_loop(&discord, interval).await });
    }

    pub async fn poll_loop(&self, discord: &serenity::Context, interval: Duration) -> ! {
        let mut interval = tokio::time::interval(interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;

            if let Err(e) = catch_panic(self.poll(discord)).await {
                tracing::error!("polling: {e:#}")
            }
        }
    }

    pub async fn poll(&self, discord: &serenity::Context) -> anyhow::Result<()> {
        let feed = &self.bsky.api.app.bsky.feed;
        let timeline = feed
            .get_timeline(
                bsky_sdk::api::app::bsky::feed::get_timeline::ParametersData {
                    algorithm: None,
                    cursor: None,
                    // By default, 50
                    limit: None,
                }
                .into(),
            )
            .await
            .context("loading feed")?;

        let last_updated = sqlx::query!("SELECT last_updated FROM global")
            .fetch_one(&self.db)
            .await
            .context("reading `last_updated`")?
            .last_updated;

        let last_updated = chrono::DateTime::from_timestamp_secs(last_updated)
            .context("last updated time out of range")?
            .fixed_offset();
        let mut max_last_updated = last_updated;

        for post in timeline.data.feed {
            let uri = post.post.uri.clone();
            match catch_panic(consume_post(self, discord, last_updated, post.data)).await {
                Ok(new_last_updated) if max_last_updated < new_last_updated => {
                    max_last_updated = new_last_updated
                        .duration_round_up(TimeDelta::seconds(1))
                        .context("rounding")?;

                    let last_updated = max_last_updated.timestamp();
                    sqlx::query!("UPDATE global SET last_updated = ?", last_updated)
                        .execute(&self.db)
                        .await
                        .context("updating `last_updated`")?;
                }
                Ok(_) => {}
                Err(e) => tracing::error!("consuming {uri}: {e:#}"),
            }
        }

        Ok(())
    }
}

async fn consume_post(
    bot: &Bot,
    discord: &serenity::Context,
    last_updated: chrono::DateTime<chrono::FixedOffset>,
    mut post: bsky::feed::defs::FeedViewPostData,
) -> anyhow::Result<chrono::DateTime<chrono::FixedOffset>> {
    let record_data = replace(&mut post.post.record, bsky_sdk::api::types::Unknown::Null);
    let record_data =
        bsky_sdk::api::app::bsky::feed::post::RecordData::try_from_unknown(record_data)
            .context("extracting post record data")?;

    let created_at = *record_data.created_at.as_ref();
    if created_at <= last_updated {
        return Ok(last_updated);
    }

    let message = make_message(&mut post, record_data)?;

    struct Channel {
        channel: NonZero<u64>,
    }

    let author = &post.post.data.author;
    let author_did = author.did.as_str();
    let mut channels = sqlx::query_as!(
        Channel,
        "SELECT channel AS \"channel: _\" FROM channel_follows WHERE did = ?",
        author_did
    )
    .fetch(&bot.db);

    while let Some(channel) = channels
        .try_next()
        .await
        .context("loading subscribed channels of author")?
    {
        let channel_id = serenity::ChannelId::from(channel.channel);
        channel_id
            .send_message(discord, message.clone())
            .await
            .context("sending message")?;
    }

    Ok(created_at)
}

fn make_message(
    post: &mut bsky::feed::defs::FeedViewPostData,
    record_data: bsky::feed::post::RecordData,
) -> anyhow::Result<CreateMessage> {
    let author = &post.post.data.author;
    let handle = author.handle.as_str();
    let rkey = extract_rkey(&post.post.data.uri, &author.did)?;
    let post_url = format!("https://bsky.app/profile/{handle}/post/{rkey}");

    let mut embed_author = CreateEmbedAuthor::new(match &author.display_name {
        Some(name) => format!("{name} ({handle})"),
        None => handle.to_owned(),
    });
    embed_author = embed_author.url(format!("https://bsky.app/profile/{handle}"));
    if let Some(avatar) = &author.avatar {
        embed_author = embed_author.icon_url(avatar);
    }
    let mut embed = CreateEmbed::new()
        .author(embed_author)
        .colour(Colour(0x1183fe))
        .description(escape_markdown(&record_data.text))
        .timestamp(*record_data.created_at.as_ref())
        .url(&post_url);

    let post_embeds = post.post.data.embed.take();
    let mut images = post_embeds.into_iter().flat_map(|embed| match embed {
        bsky_sdk::api::types::Union::Refs(
            bsky::feed::defs::PostViewEmbedRefs::AppBskyEmbedImagesView(embed),
        ) => embed.data.images,
        _ => Vec::new(),
    });

    if let Some(image) = images.next() {
        embed = embed.image(image.data.fullsize);
    }

    let mut message = CreateMessage::default()
        .content(format!("<{post_url}>"))
        .embed(embed);

    for image in images {
        message = message.add_embed(CreateEmbed::new().url(&post_url).image(image.data.fullsize));
    }

    Ok(message)
}

fn escape_markdown(s: &str) -> String {
    let mut res = Vec::new();
    for byte in s.bytes() {
        if let b'*' | b'_' | b'>' | b'`' | b'[' | b'-' | b'#' = byte {
            res.push(b'\\');
        }
        res.push(byte);
    }
    String::from_utf8(res).unwrap()
}

fn extract_rkey<'a>(uri: &'a str, author_did: &Did) -> anyhow::Result<&'a str> {
    (|| {
        let uri = uri.strip_prefix("at://")?;
        let uri = uri.strip_prefix(author_did.as_str())?;
        let uri = uri.strip_prefix("/app.bsky.feed.post/")?;
        Some(uri)
    })()
    .with_context(|| format!("{uri} is not a BlueSky AtProto link"))
}

async fn catch_panic<T, F: Future<Output = anyhow::Result<T>>>(fut: F) -> anyhow::Result<T> {
    AssertUnwindSafe(fut)
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err(anyhow!("panicked")))
}

use crate::Bot;
use anyhow::Context as _;
use anyhow::anyhow;
use bsky_sdk::api::app::bsky;
use bsky_sdk::api::types::TryFromUnknown as _;
use bsky_sdk::api::types::string::Did;
use chrono::DurationRound;
use chrono::TimeDelta;
use futures_util::FutureExt;
use futures_util::TryStreamExt;
use poise::serenity_prelude as serenity;
use poise::serenity_prelude::Colour;
use poise::serenity_prelude::CreateEmbed;
use poise::serenity_prelude::CreateEmbedAuthor;
use poise::serenity_prelude::CreateMessage;
use std::fmt::Debug;
use std::mem::replace;
use std::num::NonZero;
use std::panic::AssertUnwindSafe;
use std::time::Duration;
