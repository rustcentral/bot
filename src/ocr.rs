use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::broadcast;
use twilight_gateway::Event;
use twilight_http::Client;

// Configuration for OCR image text extraction
#[derive(Debug, Deserialize)]
pub struct Configuration {
    enabled: bool,
    google_project_id: String,
    google_api_token: String,
}

// Google api is ASS ass
#[derive(Debug, Deserialize)]
struct ApiResponse {
    responses: Vec<Response>,
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(rename = "textAnnotations")]
    text_annotations: Vec<Annotation>,
}

#[derive(Debug, Deserialize)]
struct Annotation {
    description: String,
}

pub async fn ocr(
    config: Configuration,
    mut events: broadcast::Receiver<Arc<Event>>,
    http: Arc<Client>,
) {
    if !config.enabled {
        return;
    }

    loop {
        let event_msg = events.recv().await;

        let message = match event_msg.as_deref() {
            Err(broadcast::error::RecvError::Closed) => return,
            Err(_) => continue,
            Ok(Event::MessageCreate(msg)) => msg,
            Ok(_) => continue,
        };

        if message.attachments.len() == 0 {
            continue;
        }

        let mut requests: Vec<serde_json::Value> = vec![];

        for attachment in &message.attachments {
            let content_type = match &attachment.content_type {
                Some(t) => t,
                _ => &String::from(""),
            };

            if !content_type.starts_with("image/") {
                continue;
            }

            requests.push(json!({
                "image": {
                    "source": {
                        "imageUri": attachment.proxy_url.clone()
                    }
                },
                "features": [
                    {
                        "type": "TEXT_DETECTION"
                    }
                ]
            }));
        }

        if requests.len() == 0 {
            continue;
        }

        let ocr_payload = format!(
            "{{\"requests\":{}}}",
            serde_json::to_string(&requests).unwrap()
        );

        let client = reqwest::Client::new();
        let mut headers = HeaderMap::new();
        headers.append(
            "Content-Type",
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.append(
            "x-goog-user-project",
            config.google_project_id.parse().unwrap(),
        );
        headers.append(
            "Authorization",
            format!("Bearer {}", config.google_api_token)
                .parse()
                .unwrap(),
        );

        let response_result = client
            .post("https://vision.googleapis.com/v1/images:annotate")
            .body(ocr_payload)
            .headers(headers)
            .send()
            .await;

        let body = match response_result {
            Ok(response) => response.json::<ApiResponse>().await.unwrap(),
            Err(e) => {
                eprintln!("Got error during OCR; e = {e:?}");
                continue;
            }
        };

        let mut contents: Vec<String> = vec![];

        for response in body.responses {
            let description = match response.text_annotations.first() {
                Some(a) => a.description.to_string(),
                _ => return,
            };

            if !description.is_empty() {
                contents.push(description);
            }
        }

        let result = http
            .create_message(message.channel_id)
            .content(&contents.join("\n\n"))
            .reply(message.id)
            .await;

        if result.is_err() {
            tracing::warn!("Could not reply to message; id = {}", message.id);
        }
    }
}
