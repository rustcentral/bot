mod config;
mod constants;
mod error;
mod task;

use std::{path::Path, sync::Arc};
use task::ai_channel::serve_ai_channel;
use tokio::sync::broadcast;
use tracing::{info, level_filters::LevelFilter, warn};
use tracing_subscriber::{EnvFilter, filter::Directive};
use twilight_cache_inmemory::{DefaultInMemoryCache, ResourceType};
use twilight_gateway::{EventTypeFlags, Intents, Shard, ShardId, StreamExt as _};
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

    let mut shard = Shard::new(
        ShardId::ONE,
        config.token.clone(),
        Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT,
    );

    let http = Arc::new(HttpClient::builder().token(config.token).build());

    let cache = DefaultInMemoryCache::builder()
        .resource_types(ResourceType::MESSAGE)
        .build();

    // All incoming events are sent through the broadcast channel and each event is handled by every
    // task that handles events.
    let (event_tx, event_rx) = broadcast::channel(128);

    if let Some(ai_channel_config) = config.ai_channel {
        tokio::spawn(serve_ai_channel(
            ai_channel_config,
            event_rx.resubscribe(),
            http.clone(),
        ));
    } else {
        warn!("No AI channel config was set; not enabling AI channel")
    }

    info!("Listening for events");
    while let Some(item) = shard.next_event(EventTypeFlags::all()).await {
        let Ok(event) = item else {
            tracing::warn!(source = ?item.unwrap_err(), "error receiving event");

            continue;
        };

        // Update the cache with the event.
        cache.update(&event);

        // Wrap the event in Arc. Since there will be multiple receivers, this prevents the value
        // from needing to be deeply cloned for each receiver.
        let event = Arc::new(event);
        _ = event_tx.send(event);
    }

    Ok(())
}
