mod ai_channel;
mod config;
mod error;

use config::file_watch::{load_prompt, monitor_prompt};
use std::{path::Path, sync::Arc};
use tokio::{select, sync::broadcast};
use tracing::{error, info, level_filters::LevelFilter};
use tracing_subscriber::{EnvFilter, filter::Directive};
use twilight_cache_inmemory::{DefaultInMemoryCache, InMemoryCache, ResourceType};
use twilight_gateway::{
    CloseFrame, Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _,
};
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

    let shard = Shard::new(
        ShardId::ONE,
        config.token.clone(),
        Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT,
    );
    let shard_sender = shard.sender();

    let http = Arc::new(HttpClient::builder().token(config.token).build());

    let cache = Arc::new(
        DefaultInMemoryCache::builder()
            .resource_types(ResourceType::empty())
            .build(),
    );

    // All incoming events are sent through the broadcast channel and each event is handled by every
    // task that handles events.
    let (event_tx, event_rx) = broadcast::channel(16);

    info!("Serving {} AI channel(s)", config.ai_channels.len());
    for ai_channel_config in config.ai_channels {
        let (prompt_sender, prompt_receiver) =
            match load_prompt(ai_channel_config.get_prompt_path()).await {
                Ok(var) => var,
                Err(err) => {
                    tracing::error!("Unable to read channel prompt: {err}");
                    tracing::error!(
                        "Channel with id '{}' will not be activated",
                        ai_channel_config.get_channel_id()
                    );
                    continue;
                }
            };

        if let Err(err) = monitor_prompt(ai_channel_config.get_prompt_path(), prompt_sender) {
            tracing::error!(
                "Unable to watch prompt file at '{}' for channel '{}'. The channel will be active, but the prompt wont be updated unless the program is restarted.",
                ai_channel_config.get_prompt_path().display(),
                ai_channel_config.get_channel_id()
            );
            tracing::error!("{err}");
        };

        tokio::spawn(ai_channel::serve(
            ai_channel_config,
            event_rx.resubscribe(),
            http.clone(),
            prompt_receiver,
        ));
    }

    info!("Listening for events");
    select! {
        _ = handle_events(shard, cache, event_tx) => {},
        res = await_exit_signal() => {
            if let Err(err) = res {
                error!("error waiting exit signal: {err}");
            }
        },
    }
    _ = shard_sender.close(CloseFrame::NORMAL);
    Ok(())
}

/// Listen for discord events and broadcast them to all event handlers.
async fn handle_events(
    mut shard: Shard,
    cache: Arc<InMemoryCache>,
    event_tx: broadcast::Sender<Arc<Event>>,
) {
    while let Some(item) = shard.next_event(EventTypeFlags::all()).await {
        let Ok(event) = item else {
            tracing::warn!(source = ?item.unwrap_err(), "error receiving event");

            continue;
        };

        if let Event::GatewayClose(Some(info)) = &event {
            error!(code = info.code, reason = %info.reason, "Gateway connection closed");
        }

        // Update the cache with the event.
        cache.update(&event);

        // Wrap the event in Arc. Since there will be multiple receivers, this prevents the value
        // from needing to be deeply cloned for each receiver.
        let event = Arc::new(event);
        _ = event_tx.send(event);
    }
}

/// Helper function to listen for an exit signal regardless of platform.
async fn await_exit_signal() -> std::io::Result<()> {
    // This depends on the platform as docker will send a sigterm signal which does not exist on
    // non-unix platforms (like windows).
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;
        select! {
            _ = sigterm.recv() => Ok(()),
            _ = sigint.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        use tokio::signal;

        signal::ctrl_c().await
    }
}
