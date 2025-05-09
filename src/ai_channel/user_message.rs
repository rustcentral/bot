use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
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
        });

        if let Err(mpsc::error::TrySendError::Closed(_)) = res {
            return;
        }
    }
}
