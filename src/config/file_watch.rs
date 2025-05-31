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

        // Access events spam (personal experience).
        if !(event.kind.is_modify() || event.kind.is_other()) {
            continue;
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::write, time::Duration};
    use tokio::time::sleep;

    /// The text in the file must be the same as what is loaded into the prompt.
    #[tokio::test]
    async fn load_in_prompt() {
        let tempdir = tempfile::tempdir().expect("Unable to create temporary directory.");

        let mut prompt_file = tempdir.path().to_path_buf();
        prompt_file.push("prompt.txt");
        let prompt_file = prompt_file.as_path();

        write(prompt_file, "Test prompt data").expect("Unable to write dummy prompt data");

        let prompt = load_prompt(prompt_file)
            .await
            .expect("Unable to load prompt file");

        assert_eq!(
            *prompt.lock().expect("Unable to lock mutex"),
            "Test prompt data".into()
        );
    }

    /// When the prompt file is modified the in memory prompt must change within a reasonable time frame.
    #[tokio::test]
    async fn prompt_is_updated() {
        let tempdir = tempfile::tempdir().expect("Unable to create temporary directory.");

        let mut prompt_file = tempdir.path().to_path_buf();
        prompt_file.push("prompt.txt");
        let prompt_file = prompt_file.as_path();

        write(prompt_file, "Test prompt data").expect("Unable to write dummy prompt data");

        let shared_prompt = load_prompt(prompt_file)
            .await
            .expect("Unable to load prompt file");

        monitor_prompt(prompt_file, shared_prompt.clone());

        // Prevent race condition where file is written to before watcher inits.
        sleep(Duration::from_secs(1)).await;

        write(prompt_file, "New prompt data!").expect("Unable to write new prompt data");

        let mut checks = 0;
        loop {
            sleep(Duration::from_millis(100)).await;

            {
                let data = shared_prompt.lock().expect("Mutex was poisoned");
                if *data == "New prompt data!".into() {
                    break;
                }
            }
            checks += 1;
            if checks == 20 {
                panic!(
                    "The shared prompt was not updated within ~2 sec after the prompt file was updated."
                );
            }
        }
    }
}
