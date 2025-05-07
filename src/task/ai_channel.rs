use std::{collections::VecDeque, sync::Arc};

use async_openai::{
    Client as AIClient,
    config::OpenAIConfig,
    types::{
        ChatChoice, ChatCompletionRequestMessage, ChatCompletionResponseMessage,
        CreateChatCompletionRequestArgs,
    },
};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error};
use twilight_gateway::Event;
use twilight_http::Client;
use twilight_model::{
    id::{
        Id,
        marker::{ChannelMarker, UserMarker},
    },
    util::Timestamp,
};

#[derive(Debug)]
struct UserMessage {
    content: String,
    sender_name: String,
    sender_display_name: Option<String>,
    sender_id: Id<UserMarker>,
    sent_at: Timestamp,
}

pub async fn serve_ai_channel(
    api_key: String,
    api_base: String,
    model_name: String,
    channel_id: Id<ChannelMarker>,
    mut events: broadcast::Receiver<Arc<Event>>,
    http: Arc<Client>,
) {
    let client = AIClient::with_config(
        OpenAIConfig::new()
            .with_api_base(api_base)
            .with_api_key(api_key),
    );

    let (message_tx, mut message_rx) = mpsc::unbounded_channel();

    // Spawn a task to handle incoming message events and queue them in the unbounded channel
    // created above.
    tokio::spawn(async move {
        loop {
            let event = events.recv().await;
            let message = match event.as_deref() {
                Err(broadcast::error::RecvError::Closed) => return,
                Err(_) => continue,
                Ok(Event::MessageCreate(msg)) => msg,
                Ok(_) => continue,
            };

            if message.channel_id != channel_id || message.author.bot {
                continue;
            }

            _ = message_tx.send(UserMessage {
                content: message.content.clone(),
                sender_name: message.author.name.clone(),
                sender_id: message.author.id,
                sent_at: message.timestamp,
                sender_display_name: message
                    .member
                    .as_ref()
                    .map(|m| m.nick.clone())
                    .flatten()
                    .or_else(|| message.author.global_name.clone()),
            });
        }
    });

    let max_history_size = 32;
    let mut history = VecDeque::new();

    // Batch new messages together to avoid generating a separate response to each one.
    let mut new_messages = Vec::new();
    while message_rx.recv_many(&mut new_messages, 100).await > 0 {
        let system_prompt = ChatCompletionRequestMessage::System(
            include_str!("./ai_channel/system_prompt.txt").into(),
        );

        for msg in &new_messages {
            let msg = ChatCompletionRequestMessage::User(
                format!(
                    "<msg>author_name: {}\nauthor_id: {}{}\nsent_at: {}\n{}</msg>",
                    msg.sender_name,
                    match &msg.sender_display_name {
                        Some(name) => format!(" ({name})"),
                        None => String::new(),
                    },
                    msg.sender_id,
                    msg.sent_at.iso_8601(),
                    msg.content
                )
                .into(),
            );

            history.push_back(msg);
        }
        new_messages.clear();

        // Downsize the history buffer by removing some elements from the front until it is back to
        // `max_history_size`. This is to ensure all messages fit in the context window.
        let remove_from_front = history.len().saturating_sub(max_history_size);
        history.drain(0..remove_from_front);

        let messages: Vec<_> = [system_prompt]
            .into_iter()
            .chain(history.iter().map(|i| i.clone()))
            .collect();

        let request = CreateChatCompletionRequestArgs::default()
            .model(&model_name)
            .messages(messages)
            .build();
        let request = match request {
            Ok(v) => v,
            Err(err) => {
                // This should not happen, hopefully.
                error!("Failed to build request: {err}");
                continue;
            }
        };

        let response = client.chat().create(request).await;
        let response = match response {
            Ok(v) => v,
            Err(err) => {
                error!("LLM api returned an error: {err}");
                continue;
            }
        };

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
                error!("LLM response did not include message content");
                continue;
            }
        };

        history.push_back(ChatCompletionRequestMessage::Assistant(
            response_content.into(),
        ));

        if response_content.trim() == "<empty/>" {
            debug!("Model chose to not respond");
            continue;
        }

        if let Err(err) = http
            .create_message(channel_id)
            .content(&response_content)
            .await
        {
            error!("Failed to send response message: {err}");
            continue;
        }
    }
}
