pub async fn poll_loop(discord: serenity::Context, data: Data) -> ! {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;

        if let Err(e) = catch_panic(poll(&discord, &data)).await {
            tracing::error!("polling: {e:#}")
        }
    }
}

async fn poll(discord: &serenity::Context, data: &Data) -> anyhow::Result<()> {
    tracing::info!("polling…");

    let feed = &data.bsky.api.app.bsky.feed;
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
        .fetch_one(&data.db)
        .await
        .context("reading `last_updated`")?
        .last_updated;

    let last_updated = chrono::DateTime::from_timestamp_secs(last_updated)
        .context("last updated time out of range")?
        .fixed_offset();
    let mut max_last_updated = last_updated;

    for post in timeline.data.feed {
        let uri = post.post.uri.clone();
        match catch_panic(consume_post(discord, data, last_updated, post.data)).await {
            Ok(new_last_updated) if max_last_updated < new_last_updated => {
                max_last_updated = new_last_updated
                    .duration_round_up(TimeDelta::seconds(1))
                    .context("rounding")?;

                let last_updated = max_last_updated.timestamp();
                sqlx::query!("UPDATE global SET last_updated = ?", last_updated)
                    .execute(&data.db)
                    .await
                    .context("updating `last_updated`")?;
            }
            Ok(_) => {}
            Err(e) => tracing::error!("consuming {uri}: {e:#}"),
        }
    }

    Ok(())
}

async fn consume_post(
    discord: &serenity::Context,
    data: &Data,
    last_updated: chrono::DateTime<chrono::FixedOffset>,
    post: bsky::feed::defs::FeedViewPostData,
) -> anyhow::Result<chrono::DateTime<chrono::FixedOffset>> {
    let record_data =
        bsky_sdk::api::app::bsky::feed::post::RecordData::try_from_unknown(post.post.data.record)
            .context("extracting post record data")?;

    if *record_data.created_at.as_ref() <= last_updated {
        return Ok(last_updated);
    }

    let author = &post.post.data.author;
    let rkey = extract_rkey(&post.post.data.uri, &author.did)?;
    let message = format!(
        "https://bsky.app/profile/{}/post/{}",
        author.handle.as_str(),
        rkey,
    );

    struct Channel {
        channel: NonZero<u64>,
    }

    let author_did = author.did.as_str();
    let mut channels = sqlx::query_as!(
        Channel,
        "SELECT channel AS \"channel: _\" FROM channel_follows WHERE did = ?",
        author_did
    )
    .fetch(&data.db);

    while let Some(channel) = channels
        .try_next()
        .await
        .context("loading subscribed channels of author")?
    {
        let channel_id = serenity::ChannelId::from(channel.channel);
        channel_id
            .say(discord, &message)
            .await
            .context("sending message")?;
    }

    Ok(*record_data.created_at.as_ref())
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

use crate::data::Data;
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
use std::fmt::Debug;
use std::num::NonZero;
use std::panic::AssertUnwindSafe;
use std::time::Duration;
