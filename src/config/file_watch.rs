use anyhow::anyhow;
use notify::{Config, Event, RecommendedWatcher, Watcher};
use std::path::Path;
use tokio::sync::watch;

/// Reads the channel prompt into a [`watch`] channel.
///
/// The [`watch::Receiver`] will have its value updated when the channel prompt file is modified.
#[doc(alias = "read_prompt")]
pub async fn load_prompt(
    prompt_path: &Path,
) -> Result<(watch::Sender<Box<str>>, watch::Receiver<Box<str>>), std::io::Error> {
    let current_prompt = tokio::fs::read_to_string(&prompt_path)
        .await?
        .into_boxed_str();

    Ok(watch::channel(current_prompt))
}

/// Monitors the channel prompt file for changes.
///
/// # Panics
/// If this function is called from outside of a tokio runtime.
pub fn monitor_prompt(path: &Path, prompt_sender: watch::Sender<Box<str>>) -> anyhow::Result<()> {
    // Normalises the path.
    // The path is compared with to filter events later.
    let Ok(prompt_path) = path.canonicalize() else {
        return Err(anyhow!("Unable to get canonical path for channel prompt",));
    };

    let mut watcher = match RecommendedWatcher::new(
        create_event_handler(prompt_sender.clone(), prompt_path.clone().into_boxed_path()),
        Config::default(),
    ) {
        Ok(var) => var,
        Err(err) => {
            return Err(anyhow!("Unable to start watcher for channel prompt: {err}"));
        }
    };

    // Boxed to moved across threads.
    let prompt_dir: Box<Path> = match prompt_path.parent() {
        Some(parent) => parent.into(),
        None => {
            return Err(anyhow!("Unable to get directory for channel prompt"));
        }
    };

    // See watcher docs for why watching directory.
    if let Err(err) = watcher.watch(&prompt_dir, notify::RecursiveMode::NonRecursive) {
        return Err(anyhow!("Unable to start watching channel prompt: {err}"));
    };

    // Watcher needs to live for duration of program.
    tokio::spawn(async move {
        prompt_sender.closed().await;
        // Ensure task takes ownership of watcher.
        drop(watcher);
    });

    Ok(())
}

/// Creates the event handler for updating the channel prompt.
fn create_event_handler(
    sender: watch::Sender<Box<str>>,
    prompt_path: Box<Path>,
) -> impl Fn(Result<Event, notify::Error>) {
    move |event| {
        let event: Event = match event {
            Ok(var) => var,
            Err(err) => {
                tracing::error!(
                    "Error whilst watching channel prompt file '{}'",
                    prompt_path.display()
                );
                tracing::error!("{err}");
                return;
            }
        };

        // Access events spam (personal experience).
        if !(event.kind.is_modify() || event.kind.is_other()) {
            return;
        }

        // Check if the event was for this channel prompt path
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
            return;
        }

        let new_prompt = match std::fs::read_to_string(&prompt_path) {
            Ok(var) => var.into_boxed_str(),
            Err(_) => todo!(),
        };

        sender.send_modify(|prompt| *prompt = new_prompt);

        tracing::info!(
            "Updated channel prompt for file at '{}'",
            prompt_path.display()
        );
    }
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

        let (_, prompt_receiver) = load_prompt(prompt_file)
            .await
            .expect("Unable to load prompt file");

        assert_eq!(*prompt_receiver.borrow(), "Test prompt data".into());
    }

    /// When the prompt file is modified the in memory prompt must change within a reasonable time frame.
    #[tokio::test]
    async fn prompt_is_updated() {
        let tempdir = tempfile::tempdir().expect("Unable to create temporary directory.");

        let mut prompt_file = tempdir.path().to_path_buf();
        prompt_file.push("prompt.txt");
        let prompt_file = prompt_file.as_path();

        write(prompt_file, "Test prompt data").expect("Unable to write dummy prompt data");

        let (prompt_sender, prompt_receiver) = load_prompt(prompt_file)
            .await
            .expect("Unable to load prompt file");

        monitor_prompt(prompt_file, prompt_sender).expect("Unable to monitor channel prompt");

        // Prevent race condition where file is written to before watcher inits.
        sleep(Duration::from_secs(1)).await;

        write(prompt_file, "New prompt data!").expect("Unable to write new prompt data");

        let mut checks = 0;
        loop {
            sleep(Duration::from_millis(100)).await;

            if *prompt_receiver.borrow() == "New prompt data!".into() {
                break;
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
