mod user_message;

use std::{collections::VecDeque, sync::Arc, time::Duration};

use anyhow::Context;
use async_openai::{
    Client as AIClient,
    config::OpenAIConfig,
    types::{
        ChatChoice, ChatCompletionRequestMessage, ChatCompletionResponseMessage,
        CreateChatCompletionRequestArgs,
    },
};
use serde::Deserialize;
use tokio::{
    sync::{broadcast, mpsc},
    time::{Instant, sleep_until},
};
use tracing::{debug, error};
use twilight_gateway::Event;
use twilight_http::Client;
use twilight_model::id::{Id, marker::ChannelMarker};
use user_message::queue_messages;

use crate::error::send_error_msg;

#[derive(Debug, Deserialize)]
pub struct Configuration {
    channel_id: Id<ChannelMarker>,
    llm_api_key: String,
    /// The base API endpoint to use. If not set the OpenAI API will be used.
    llm_api_base: Option<String>,
    model_name: String,
    /// The maximum amount of messages to include as history when generating a response. This does
    /// *not* include the system prompt.
    #[serde(default = "default_max_history_size")]
    max_history_size: u32,
}

fn default_max_history_size() -> u32 {
    32
}

/// Runs the main AI channel logic.
pub async fn serve(
    config: Configuration,
    events: broadcast::Receiver<Arc<Event>>,
    http: Arc<Client>,
) {
    let mut llm_config = OpenAIConfig::new().with_api_key(config.llm_api_key);
    if let Some(api_base) = config.llm_api_base {
        llm_config = llm_config.with_api_base(api_base);
    }
    let llm_client = AIClient::with_config(llm_config).with_backoff(
        backoff::ExponentialBackoffBuilder::new()
            .with_max_elapsed_time(Some(Duration::from_secs(5)))
            .build(),
    );

    let max_history_size = config.max_history_size as usize;
    let (message_tx, mut message_rx) = mpsc::channel(max_history_size / 2);

    // Spawn a task to handle incoming message events and queue them in the channel above.
    tokio::spawn(queue_messages(events, message_tx, config.channel_id));

    let mut last_response_time = Instant::now();
    let mut last_error_response = None;
    let mut history = VecDeque::new();

    // Batch new messages together to avoid generating a separate response to each one.
    let mut new_messages = Vec::new();
    loop {
        // Wait to avoid getting rate limited by the LLM endpoint.
        // TODO: this could be handled better.
        sleep_until(last_response_time + Duration::from_millis(1500)).await;

        let recv_amt = message_rx
            .recv_many(&mut new_messages, max_history_size)
            .await;

        if recv_amt == 0 {
            // The message ingestion channel has closed, gracefully shut down this task.
            break;
        }

        let system_prompt = ChatCompletionRequestMessage::System(
            include_str!("./ai_channel/system_prompt.txt").into(),
        );

        for msg in &new_messages {
            let msg = ChatCompletionRequestMessage::User(msg.format_message().into());

            history.push_back(msg);
        }
        new_messages.clear();

        // Downsize the history buffer by removing some elements from the front until it is back to
        // `max_history_size`. This is to ensure all messages fit in the context window.
        let remove_from_front = history.len().saturating_sub(max_history_size);
        // TODO: count history in tokens rather amount of messages.
        history.drain(0..remove_from_front);

        let messages: Vec<_> = [system_prompt]
            .into_iter()
            .chain(history.iter().map(|i| i.clone()))
            .collect();

        let response = generate_response(&llm_client, &config.model_name, messages).await;
        last_response_time = Instant::now();

        // Delete the previous error message. This should happen both if there is a new error
        // message or there is another error.
        if let Some(prev_err_msg_id) = last_error_response {
            let http2 = http.clone();
            tokio::spawn(async move {
                if let Err(err) = http2
                    .delete_message(config.channel_id, prev_err_msg_id)
                    .await
                {
                    error!("Failed to delete previous error message: {err}");
                }
            });

            last_error_response = None;
        }

        let mut response_content = match response {
            Ok(v) => v,
            Err(err) => {
                error!("Error creating response: {err:?}");

                // Log the error in the channel.
                let err_msg = send_error_msg(
                    &http,
                    config.channel_id,
                    &format!("Something went wrong while generating a response\n```\n{err}\n```"),
                )
                .await;

                if let Some(err_msg) = err_msg {
                    last_error_response = Some(err_msg.id);
                };
                continue;
            }
        };
        // Take only the first 2000 characters to stay within the discord character limit.
        response_content.truncate(
            response_content
                .char_indices()
                .take(2000)
                .map(|v| v.0 + v.1.len_utf8())
                .last()
                .unwrap_or(0),
        );

        if response_content.contains("<empty/>") {
            debug!("Model chose to not respond");
            continue;
        }

        history.push_back(ChatCompletionRequestMessage::Assistant(
            response_content.as_str().into(),
        ));

        if let Err(err) = http
            .create_message(config.channel_id)
            .content(&response_content)
            .await
        {
            error!("Failed to send response message: {err}");
            continue;
        }
    }

    // Don't clutter the channel with lots of error messages.
    if let Some(msg_id) = last_error_response {
        _ = http.delete_message(config.channel_id, msg_id).await;
    }
}

/// Sent by the model in response to a chat history.
///
/// A custom type is used here as some (gemini *caugh caugh*) APIs dont return all fields.
#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

/// Send the chat history to the LLM api and generate a response based on this history.
async fn generate_response(
    client: &AIClient<OpenAIConfig>,
    model_name: &str,
    history: Vec<ChatCompletionRequestMessage>,
) -> anyhow::Result<String> {
    let request = CreateChatCompletionRequestArgs::default()
        .model(model_name)
        .max_tokens(400u32)
        .messages(history)
        .build()
        .context("Failed to build request")?;

    let response: ChatCompletionResponse = client
        .chat()
        .create_byot(request)
        .await
        .context("LLM api returned an error")?;

    let response_content = match response.choices.first() {
        Some(ChatChoice {
            message:
                ChatCompletionResponseMessage {
                    content: Some(content),
                    ..
                },
            ..
        }) => content.as_str(),
        _ => {
            anyhow::bail!("LLM response did not include message content");
        }
    };

    Ok(response_content.to_string())
}
