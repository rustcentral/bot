mod config;

use std::{path::Path, sync::Arc};
use tracing::{info, instrument, level_filters::LevelFilter};
use tracing_subscriber::{EnvFilter, filter::Directive};
use twilight_cache_inmemory::{DefaultInMemoryCache, ResourceType};
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _};
use twilight_http::Client as HttpClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(Directive::from(LevelFilter::INFO))
                .from_env_lossy(),
        )
        .init();

    let config = config::Configuration::read_with_env("CONFIG_PATH", [Path::new("bot.toml")])?;

    let mut shard = Shard::new(ShardId::ONE, config.token.clone(), Intents::all());

    let http = Arc::new(HttpClient::builder().token(config.token).build());

    let cache = DefaultInMemoryCache::builder()
        .resource_types(ResourceType::MESSAGE)
        .build();

    info!("Listening for events");
    while let Some(item) = shard.next_event(EventTypeFlags::all()).await {
        let Ok(event) = item else {
            tracing::warn!(source = ?item.unwrap_err(), "error receiving event");

            continue;
        };

        // Update the cache with the event.
        cache.update(&event);

        tokio::spawn(handle_event(event, Arc::clone(&http)));
    }

    Ok(())
}

#[instrument(skip_all, fields(event = ?event.kind()))]
async fn handle_event(event: Event, _http: Arc<HttpClient>) -> anyhow::Result<()> {
    match event {
        _ => {}
    }

    Ok(())
}
