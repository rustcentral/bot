use std::{io::Cursor, sync::Arc};

use async_openai::types::{
    ChatCompletionRequestMessageContentPartImage, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart,
    ImageDetail, ImageUrl,
};
use base64::{Engine, prelude::BASE64_STANDARD};
use image::{GenericImageView, ImageFormat, ImageReader, imageops::FilterType};
use tokio::sync::{broadcast, mpsc};
use tracing::error;
use twilight_gateway::Event;
use twilight_model::{
    id::{
        Id,
        marker::{ChannelMarker, MessageMarker, UserMarker},
    },
    util::Timestamp,
};

#[derive(Debug)]
pub struct UserMessage {
    pub message_id: Id<MessageMarker>,
    pub reply_to: Option<Id<MessageMarker>>,
    pub content: String,
    pub sender_name: String,
    pub sender_display_name: Option<String>,
    pub sender_id: Id<UserMarker>,
    pub sent_at: Timestamp,
    pub images: Vec<String>,
}

impl UserMessage {
    /// Serialize the message into the format expected by the LLM.
    pub fn format_message(&self) -> String {
        format!(
            "<msg>message_id: {}\n{}author_name: {}\nauthor_id: {}{}\nsent_at: {}\n{}</msg>",
            self.message_id,
            match self.reply_to {
                Some(id) => format!("repling_to: {id}\n"),
                None => String::new(),
            },
            self.sender_name,
            match &self.sender_display_name {
                Some(name) => format!(" ({name})"),
                None => String::new(),
            },
            self.sender_id,
            self.sent_at.iso_8601(),
            self.content
        )
    }

    /// Encode the message into the format excpected by the LLM api.
    pub async fn as_chat_completion_message(
        &self,
        config: &super::Configuration,
    ) -> ChatCompletionRequestUserMessage {
        if !config.image_support {
            // Not using the content parts ensures maximum compatibility.
            return self.format_message().into();
        }

        let mut content = vec![ChatCompletionRequestUserMessageContentPart::Text(
            self.format_message().into(),
        )];

        for image in &self.images {
            let image_b64 = match b64_encode_image(image, config.max_image_size).await {
                Ok(v) => v,
                Err(err) => {
                    // Don't propagate the error up: there are a lot of reasons why encoding the
                    // image could go wrong, for example when the users povides an invalid image. It
                    // seems better to let the chat continue without errors in this case.
                    error!("Failed to encode image: {err:?}");
                    continue;
                }
            };

            content.push(ChatCompletionRequestUserMessageContentPart::ImageUrl(
                ChatCompletionRequestMessageContentPartImage {
                    image_url: ImageUrl {
                        url: format!("data:image/jpeg;base64,{image_b64}"),
                        // Images can be very expensive in terms of tokens.
                        detail: Some(ImageDetail::Low),
                    },
                },
            ));
        }

        ChatCompletionRequestUserMessage {
            content: ChatCompletionRequestUserMessageContent::Array(content),
            ..Default::default()
        }
    }
}

/// Queue incoming messages in a certain discord channel into a queue channel.
pub async fn queue_messages(
    mut events: broadcast::Receiver<Arc<Event>>,
    queue: mpsc::Sender<UserMessage>,
    channel_id: Id<ChannelMarker>,
) {
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

        let res = queue.try_send(UserMessage {
            message_id: message.id,
            reply_to: message.reference.as_ref().map(|r| r.message_id).flatten(),
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
            images: message
                .attachments
                .iter()
                .filter_map(|a| {
                    let extension = a.filename.rsplit('.').next();
                    match extension {
                        Some("jpeg" | "jpg" | "png" | "webp") => Some(a.url.clone()),
                        _ => None,
                    }
                })
                .collect(),
        });

        if let Err(mpsc::error::TrySendError::Closed(_)) = res {
            return;
        }
    }
}

async fn b64_encode_image(image_url: &str, max_dim: u32) -> anyhow::Result<String> {
    let image_bytes = reqwest::get(image_url).await?.bytes().await?;
    let img = ImageReader::new(Cursor::new(image_bytes))
        .with_guessed_format()?
        .decode()?;

    // Make the image smaller while preserving the aspect ratio to save on tokens.
    let img = if img.dimensions().0 > max_dim || img.dimensions().1 > max_dim {
        img.resize(max_dim, max_dim, FilterType::Triangle)
    } else {
        img
    };

    let mut img_bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut img_bytes), ImageFormat::Jpeg)?;

    Ok(BASE64_STANDARD.encode(img_bytes))
}
