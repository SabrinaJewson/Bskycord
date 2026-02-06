#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    dotenv().context("reading .env")?;

    let bsky_config_file = env::var_os("BSKY_CONFIG_FILE").context("`BSKY_CONFIG_FILE` not set")?;
    let db_url = env::var("DATABASE_URL").context("reading `DATABASE_URL`")?;
    let discord_token = env::var("DISCORD_TOKEN").context("reading `DISCORD_TOKEN`")?;

    let bot = match bskycord::Bot::new(bsky_config_file.as_ref(), &db_url).await? {
        Ok(bot) => bot,
        Err(needs_credentials) => {
            let identifier = env::var("BSKY_IDENTIFIER").context("reading `BSKY_IDENTIFIER`")?;
            let password = env::var("BSKY_PASSWORD").context("reading `BSKY_PASSWORD`")?;
            needs_credentials.finish(&identifier, &password).await?
        }
    };

    let poise = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: bskycord::commands::<bskycord::Bot, anyhow::Error>().collect(),
            ..Default::default()
        })
        .setup(|cx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(cx, &framework.options().commands)
                    .await
                    .context("registering commands")?;

                bot.start_poll(cx.clone(), Duration::from_secs(60));

                Ok(bot)
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

use anyhow::Context as _;
use bskycord::poise;
use dotenvy::dotenv;
use poise::serenity_prelude as serenity;
use std::env;
use std::time::Duration;
