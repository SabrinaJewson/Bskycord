#[derive(Clone)]
pub struct Data {
    pub db: SqlitePool,
    pub bsky: BskyAgent,
}

impl Data {
    pub async fn new(bsky_config: &Path, db_url: &str) -> anyhow::Result<Self> {
        Ok(Self {
            db: connect_to_db(db_url).await?,
            bsky: make_bsky_agent(bsky_config).await?,
        })
    }
}

async fn make_bsky_agent(store: &Path) -> anyhow::Result<BskyAgent> {
    let file_store = bsky_sdk::agent::config::FileStore::new(store);

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
        Ok(agent) => return Ok(agent),
        Err(e) => tracing::error!("loading config from {}: {e:#}", store.display()),
    }

    let identifier = env::var("BSKY_IDENTIFIER").context("reading `BSKY_IDENTIFIER`")?;
    let password = env::var("BSKY_PASSWORD").context("reading `BSKY_PASSWORD`")?;

    let agent = BskyAgent::builder()
        .build()
        .await
        .context("making new bsky agent")?;
    agent
        .login(&identifier, &password)
        .await
        .context("logging in to BlueSky")?;
    agent
        .to_config()
        .await
        .save(&file_store)
        .await
        .with_context(|| format!("saving config to {}", store.display()))?;
    Ok(agent)
}

async fn connect_to_db(db_url: &str) -> anyhow::Result<SqlitePool> {
    let db_opts = SqliteConnectOptions::from_str(db_url)
        .with_context(|| format!("parsing DB URL `{db_url}`"))?
        .create_if_missing(true);
    let db = SqlitePool::connect_with(db_opts)
        .await
        .with_context(|| format!("connecting to {db_url}"))?;

    sqlx::migrate!()
        .run(&db)
        .await
        .context("running migrations")?;

    Ok(db)
}

use anyhow::Context as _;
use bsky_sdk::BskyAgent;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::env;
use std::path::Path;
use std::str::FromStr;
