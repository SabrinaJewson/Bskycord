#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    dotenv().context("reading .env")?;

    let bsky_config_file = env::var_os("BSKY_CONFIG_FILE").context("`BSKY_CONFIG_FILE` not set")?;
    let db_url = env::var("DATABASE_URL").context("reading `DATABASE_URL`")?;
    let discord_token = env::var("DISCORD_TOKEN").context("reading `DISCORD_TOKEN`")?;

    let data = Data::new(bsky_config_file.as_ref(), &db_url).await?;

    let poise = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all(),
            ..Default::default()
        })
        .setup(|cx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(cx, &framework.options().commands)
                    .await
                    .context("registering commands")?;

                tokio::spawn(poll::poll_loop(cx.clone(), data.clone()));

                Ok(data)
            })
        })
        .build();

    serenity::ClientBuilder::new(discord_token, serenity::GatewayIntents::empty())
        .framework(poise)
        .await
        .context("creating Discord client")?
        .start()
        .await
        .context("starting Discord client")?;

    Ok(())
}

mod commands;
mod data;
mod poll;

use crate::data::Data;
use anyhow::Context as _;
use dotenvy::dotenv;
use poise::serenity_prelude as serenity;
use std::env;
