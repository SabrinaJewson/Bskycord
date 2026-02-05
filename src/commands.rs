pub fn all() -> Vec<poise::Command<Data, DisplayAsAlt<anyhow::Error>>> {
    vec![follow(), unfollow(), follows()]
}

type Context<'a> = poise::Context<'a, Data, DisplayAsAlt<anyhow::Error>>;

/// Follow a BlueSky profile in the channel
#[poise::command(slash_command, default_member_permissions = "ADMINISTRATOR", ephemeral)]
async fn follow(
    cx: Context<'_>,
    #[description = "The profile to follow"] profile: AtIdentifierWrapper,
) -> Result<(), DisplayAsAlt<anyhow::Error>> {
    let guild = cx.guild_id().map(i64::from);
    let channel = i64::from(cx.channel_id());

    let actor = &cx.data().bsky.api.app.bsky.actor;
    let profile = actor
        .get_profile(
            bsky_sdk::api::app::bsky::actor::get_profile::ParametersData {
                actor: profile.0.clone(),
            }
            .into(),
        )
        .await
        .with_context(|| format!("reading profile of {}", profile.0.as_ref()))?;

    let mut tx = cx.data().db.begin().await.context("starting transaction")?;

    let did = profile.did.as_str();
    let handle = profile.handle.as_str();

    let exists = sqlx::query!("SELECT 1 AS \"__: i64\" FROM follows WHERE did = ?", did)
        .fetch_optional(&mut *tx)
        .await
        .context("checking if user is already followed")?
        .is_some();

    if !exists {
        let followed = cx
            .data()
            .bsky
            .create_record(bsky_sdk::api::app::bsky::graph::follow::RecordData {
                created_at: bsky_sdk::api::types::string::Datetime::now(),
                subject: profile.did.clone(),
            })
            .await
            .with_context(|| format!("following {handle}"))?;
        tracing::info!("followed {handle}");

        let uri = &followed.uri;
        sqlx::query!("INSERT INTO follows VALUES (NULL, ?, ?)", did, uri)
            .execute(&mut *tx)
            .await
            .context("recording follow")?;
    }

    sqlx::query!(
        "INSERT INTO channel_follows VALUES (NULL, ?, ?, ?, ?)",
        did,
        handle,
        guild,
        channel
    )
    .execute(&mut *tx)
    .await
    .context("following in channel")?;

    tx.commit().await.context("commit")?;

    cx.say(format!("Followed {handle} in <#{channel}>"))
        .await
        .context("responding")?;

    Ok(())
}

/// Unfollow a BlueSky profile in the channel
#[poise::command(slash_command, default_member_permissions = "ADMINISTRATOR", ephemeral)]
async fn unfollow(
    cx: Context<'_>,
    #[description = "The profile to unfollow"]
    #[autocomplete = followed_profiles]
    profile: DidWrapper,
) -> Result<(), DisplayAsAlt<anyhow::Error>> {
    let channel = i64::from(cx.channel_id());

    let did = profile.0.as_str();

    let result = sqlx::query!(
        "DELETE FROM channel_follows WHERE did = ? AND channel = ?",
        did,
        channel
    )
    .execute(&cx.data().db)
    .await
    .context("deleting follow")?;

    let data = cx.data().clone();

    cx.say(match result.rows_affected() {
        0 => format!("{did} was not followed to begin with"),
        _ => format!("Successfully unfollowed {did}"),
    })
    .await
    .context("responding")?;

    let mut tx = data.db.begin().await.context("starting transaction")?;

    let other_channel_follows = sqlx::query!(
        "SELECT EXISTS(SELECT 1 FROM channel_follows WHERE did = ?) AS ex",
        did
    )
    .fetch_one(&mut *tx)
    .await
    .context("checking for other channel follows")?
    .ex != 0;

    if !other_channel_follows {
        let row = sqlx::query!("DELETE FROM follows WHERE did = ? RETURNING uri", did)
            .fetch_optional(&mut *tx)
            .await
            .context("recording removed follow")?;

        if let Some(row) = row {
            data.bsky
                .delete_record(row.uri)
                .await
                .with_context(|| format!("unfollowing {did}"))?;
            tracing::info!("unfollowed {did}");

            tx.commit().await.context("commit")?;
        }
    }

    Ok(())
}

/// Show follows in this guild
#[poise::command(slash_command, ephemeral)]
async fn follows(cx: Context<'_>) -> Result<(), DisplayAsAlt<anyhow::Error>> {
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    struct Follow {
        handle: String,
        channel: NonZero<u64>,
    }

    let guild = cx.guild_id().map(i64::from);
    let channel = i64::from(cx.channel_id());

    let mut follows =
        match &guild {
            Some(guild) => {
                sqlx::query_as!(
                    Follow,
                    "SELECT handle, channel AS \"channel: _\" FROM channel_follows WHERE guild = ?",
                    *guild
                )
                .fetch_all(&cx.data().db)
                .await
            }
            None => sqlx::query_as!(
                Follow,
                "SELECT handle, channel AS \"channel: _\" FROM channel_follows WHERE channel = ?",
                channel
            )
            .fetch_all(&cx.data().db)
            .await,
        }
        .context("listing follows")?;

    follows.sort_unstable();

    let mut msg = String::new();

    for follow in follows {
        writeln!(msg, "- {} — <#{}>", follow.handle, follow.channel).unwrap();
    }

    if msg.is_empty() {
        msg.push_str("No follows in this guild");
    }

    cx.say(msg).await.context("responding")?;

    Ok(())
}

async fn followed_profiles(
    cx: Context<'_>,
    term: &str,
) -> impl Iterator<Item = AutocompleteChoice> {
    let res = async {
        let channel = i64::from(cx.channel_id());
        let mut choices = Vec::new();
        let pattern = format!("%{term}%");
        let mut stream = sqlx::query!(
            "SELECT handle, did FROM channel_follows WHERE channel = ? AND handle LIKE ?",
            channel,
            pattern,
        )
        .fetch(&cx.data().db);
        while let Some(row) = stream
            .try_next()
            .await
            .context("reading followed profiles")?
        {
            choices.push((row.handle, row.did));
        }
        choices.sort_unstable();
        anyhow::Ok(choices)
    }
    .await;

    let choices = match res {
        Ok(choices) => choices,
        Err(e) => {
            tracing::error!("loading choices: {e:#}");
            Vec::new()
        }
    };
    choices
        .into_iter()
        .map(|(handle, did)| AutocompleteChoice::new(handle, did))
}

struct AtIdentifierWrapper(AtIdentifier);

impl FromStr for AtIdentifierWrapper {
    type Err = ErrorWrapper;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        AtIdentifier::from_str(s).map(Self).map_err(|e| {
            ErrorWrapper(anyhow!(e).context(format!("{s} is not a valid DID or handle")))
        })
    }
}

struct DidWrapper(Did);

impl FromStr for DidWrapper {
    type Err = ErrorWrapper;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Did::from_str(s)
            .map(Self)
            .map_err(|e| ErrorWrapper(anyhow!(e).context(format!("{s} is not a valid DID"))))
    }
}

struct ErrorWrapper(anyhow::Error);

impl Debug for ErrorWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <anyhow::Error as Debug>::fmt(&self.0, f)
    }
}

impl Display for ErrorWrapper {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        <anyhow::Error as Display>::fmt(&self.0, f)
    }
}

impl Error for ErrorWrapper {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.source()
    }
}

pub struct DisplayAsAlt<T>(T);

impl<T: Display> Debug for DisplayAsAlt<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.0)
    }
}
impl<T: Display> Display for DisplayAsAlt<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl<T> From<T> for DisplayAsAlt<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

use crate::data::Data;
use anyhow::Context as _;
use anyhow::anyhow;
use bsky_sdk::api::types::string::AtIdentifier;
use bsky_sdk::api::types::string::Did;
use futures_util::TryStreamExt;
use poise::serenity_prelude::AutocompleteChoice;
use std::error::Error;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Write as _;
use std::num::NonZero;
use std::str::FromStr;
