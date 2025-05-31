use notify::{Config, Event, RecommendedWatcher, Watcher};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// Shares the system prompt across threads whilst allowing it to be updated.
pub type SharedPrompt = Arc<Mutex<Box<str>>>;

/// Reads the system prompt.
#[doc(alias = "read_prompt")]
pub async fn load_prompt(prompt_path: &Path) -> Result<SharedPrompt, std::io::Error> {
    let current_prompt = tokio::fs::read_to_string(&prompt_path).await?;
    Ok(Arc::new(Mutex::new(current_prompt.into_boxed_str())))
}

/// Monitors the system prompt file for changes and updates the [`SharedPrompt`] accordingly.
///
/// # Panics
/// If this function is called from outside of a tokio runtime.
pub fn monitor_prompt(prompt_path: &Path, shared_prompt: SharedPrompt) {
    // Normalises the path.
    // The path is compared with to filter events later.
    let Ok(prompt_path) = prompt_path.canonicalize() else {
        tracing::error!("Unable to get canonical path for system prompt.");
        tracing::error!("The system prompt will only be updated when the program is restarted.");
        return;
    };

    let (sender, receiver) = mpsc::unbounded_channel();

    watch_prompt(sender, prompt_path.as_path());

    tokio::spawn(update_prompt(
        receiver,
        shared_prompt.clone(),
        prompt_path.into_boxed_path(),
    ));
}

/// Updates the [`SharedPrompt`] if the system prompt file has been modified.
async fn update_prompt(
    mut receiver: UnboundedReceiver<Result<Event, notify::Error>>,
    prompt: Arc<Mutex<Box<str>>>,
    prompt_path: Box<Path>,
) {
    while let Some(event) = receiver.recv().await {
        let event: Event = match event {
            Ok(var) => var,
            Err(err) => {
                tracing::error!("Error whilst watching system prompt file: {err}");
                continue;
            }
        };

        // Check if the event was for the system prompt path
        let for_prompt_file = event
            .paths
            .iter()
            .filter(|path| {
                path.canonicalize()
                    .ok()
                    .is_some_and(|path| *path == *prompt_path)
            })
            .count()
            != 0;

        if !for_prompt_file {
            continue;
        }

        let new_prompt = match tokio::fs::read_to_string(&prompt_path).await {
            Ok(var) => var.into_boxed_str(),
            Err(_) => todo!(),
        };

        // Keep mutex in own scope so guard is dropped quickly.
        {
            let Ok(mut data) = prompt.lock() else {
                tracing::error!("Internal prompt is invalid");
                continue;
            };

            *data = new_prompt;
        }

        tracing::info!("Updated system prompt");
    }
}

/// Watches the system prompt file for changes.
fn watch_prompt(sender: UnboundedSender<Result<Event, notify::Error>>, prompt_path: &Path) {
    let event_handler = move |event| {
        if sender.send(event).is_err() {
            tracing::error!("Unable to send prompt information internally.");
            tracing::error!("Prompt will not be updated.");
        }
    };

    let mut watcher = match RecommendedWatcher::new(event_handler, Config::default()) {
        Ok(var) => var,
        Err(err) => {
            tracing::error!("Unable to start watcher for prompt: {err}");
            tracing::error!(
                "The system prompt will only be updated when the program is restarted."
            );
            return;
        }
    };

    // Boxed to moved across threads.
    let prompt_dir: Box<Path> = match prompt_path.parent() {
        Some(parent) => parent.into(),
        None => {
            tracing::error!("Unable to get directory for prompt.");
            return;
        }
    };

    // Watcher needs to live for duration of program.
    // This wasn't documented anywhere :/
    tokio::spawn(async move {
        // See watcher docs for why watching directory.
        if let Err(err) = watcher.watch(&prompt_dir, notify::RecursiveMode::NonRecursive) {
            tracing::error!("Unable to watch system prompt: {err}");
            tracing::error!(
                "The system prompt will only be updated when the program is restarted."
            );
        };

        // Watcher needs to live for duration of program.
        // If you remove this it *will* break.
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}
