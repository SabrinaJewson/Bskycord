#[derive(Clone)]
pub struct Bot {
    pub(crate) db: SqlitePool,
    pub(crate) bsky: BskyAgent,
}

impl Bot {
    pub async fn new(
        bsky_config: &Path,
        db_url: &str,
    ) -> anyhow::Result<Result<Self, NeedsCredentials>> {
        let db = connect_to_db(db_url).await?;
        let file_store = bsky_sdk::agent::config::FileStore::new(bsky_config);

        match async {
            let config = bsky_sdk::agent::config::Config::load(&file_store)
                .await
                .context("reading config file")?;

            BskyAgent::builder()
                .config(config)
                .build()
                .await
                .context("making bsky agent with config")
        }
        .await
        {
            Ok(bsky) => Ok(Ok(Self { db, bsky })),
            Err(e) => {
                tracing::error!("loading config from {}: {e:#}", bsky_config.display());
                Ok(Err(NeedsCredentials { db, file_store }))
            }
        }
    }
}

impl Debug for Bot {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bot").field("db", &self.db).finish()
    }
}

pub struct NeedsCredentials {
    db: SqlitePool,
    file_store: bsky_sdk::agent::config::FileStore,
}

impl NeedsCredentials {
    pub async fn finish(self, identifier: &str, password: &str) -> anyhow::Result<Bot> {
        let bsky = BskyAgent::builder()
            .build()
            .await
            .context("making new bsky agent")?;
        bsky.login(&identifier, &password)
            .await
            .context("logging in to BlueSky")?;
        bsky.to_config()
            .await
            .save(&self.file_store)
            .await
            .context("saving config")?;
        Ok(Bot { db: self.db, bsky })
    }
}

async fn connect_to_db(db_url: &str) -> anyhow::Result<SqlitePool> {
    let db_opts = SqliteConnectOptions::from_str(db_url)
        .with_context(|| format!("parsing DB URL `{db_url}`"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);

    let db = SqlitePool::connect_with(db_opts)
        .await
        .with_context(|| format!("connecting to {db_url}"))?;

    sqlx::migrate!("../migrations")
        .run(&db)
        .await
        .context("running migrations")?;

    Ok(db)
}

use anyhow::Context as _;
use bsky_sdk::BskyAgent;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::path::Path;
use std::str::FromStr;
