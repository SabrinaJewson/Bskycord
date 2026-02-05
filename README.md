# Bluesky bot

A simple Discord bot that polls Bluesky and forwards posts to a Discord channel.

Run with `cargo run`. Env vars (will be read from `.env`):
- `BSKY_CONFIG_FILE`: Path to a JSON file where Bluesky session data is stored.
- `DATABASE_URL`: Sqlite database URL, e.g. `sqlite:path/to/db`.
- `DISCORD_TOKEN`: Discord token.

If `BSKY_CONFIG_FILE` does not exist, it will be created when these env vars are set:
- `BSKY_IDENTIFIER`: Your Bluesky username.
- `BSKY_PASSWORD`: Your Bluesky password.

Make a dedicated account for this because bot makes use of which accounts you follow.

Commands:
- `/follow profile`: Follow the given profile in the current channel.
- `/unfollow profile`: Unfollow the given profile in the current channel.
- `/follows`: Show all follows in the current guild.
