use tracing::error;
use twilight_http::Client;
use twilight_model::{
    channel::Message,
    id::{Id, marker::ChannelMarker},
};
use twilight_util::builder::embed::EmbedBuilder;

/// Utility function to send an error message in a discord channel.
///
/// Logs any errors that may occur while sending the message. When successful, returns the newly
/// created message.
pub async fn send_error_msg(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    message: &str,
) -> Option<Message> {
    let res = http
        .create_message(channel_id)
        .embeds(&[EmbedBuilder::new()
            .color(0xff_7f_7f)
            .description(message)
            .build()])
        .await;
    let res = match res {
        Ok(res) => res,
        Err(err) => {
            error!("Failed to create error message: {err}");
            return None;
        }
    };

    match res.model().await {
        Ok(res) => return Some(res),
        Err(err) => {
            error!("Failed to deserialize message creation response: {err}");
            return None;
        }
    };
}
