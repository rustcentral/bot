use regex::Regex;
use serde::{
    Deserialize,
    de::{self, Deserializer, Error},
};

use std::{borrow::Cow, collections::HashMap, sync::Arc};
use tokio::sync::broadcast::{Receiver, error::RecvError};
use tracing::debug;
use twilight_gateway::Event;
use twilight_http::Client;
use twilight_model::{
    gateway::{Intents, payload::incoming::MemberUpdate},
    guild::Member,
    id::{Id, marker::RoleMarker},
};

fn deserialize_regex<'de, D>(deserializer: D) -> Result<Regex, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = de::Deserialize::deserialize(deserializer)?;
    Regex::new(&s).map_err(Error::custom)
}

#[derive(Debug, Deserialize)]
#[serde(tag = "strategy", rename_all = "lowercase")]
pub enum ChangeNameUsing {
    // Special characters will be transformed to ascii or removed
    // A character mapping can be provided which is applied after deunicode has run
    Deunicode {
        #[serde(default)]
        mapping: HashMap<char, char>,
    },
    // A fixed name will be assigned to the user
    // To set this name specify the new_name property
    Fixed {
        new_name: String,
    },
}

#[derive(Debug, serde::Deserialize)]
pub struct Configuration {
    /// Name format that triggers anti-hoisting
    /// e.g. `^[A-Za-z]{2,}`
    /// Be aware that regex matching the resulting nickname, may create unnecessary triggers
    #[serde(deserialize_with = "deserialize_regex")]
    pub trigger: Regex,
    // Specifies how the renaming is handled
    pub change_name_using: ChangeNameUsing,
    /// Roles that are able to bypass anti-hoisting'
    #[serde(default)]
    pub ignore_roles: Vec<Id<RoleMarker>>,
}

pub struct AntiHoisting {}

impl AntiHoisting {
    pub const INTENTS: Intents = Intents::GUILD_MEMBERS;

    /// run the main anti-hoisting logic
    pub async fn serve(config: Configuration, mut events: Receiver<Arc<Event>>, http: Arc<Client>) {
        loop {
            let event = events.recv().await;

            // match events where a member's name is hoisted
            let hoisted_member = match event.as_deref() {
                Err(RecvError::Closed) => return,
                Err(_) => continue,
                Ok(Event::MemberUpdate(m))
                    if m.nick
                        .as_ref()
                        .is_some_and(|nick| AntiHoisting::is_hoisted(&config.trigger, nick)) =>
                {
                    m
                }
                Ok(_) => continue,
            };

            // skip member with a role that bypasses anti-hoisting
            if hoisted_member
                .roles
                .iter()
                .any(|role| config.ignore_roles.contains(role))
            {
                debug!(
                    name = %hoisted_member.user.name,
                    "Member has a role that bypasses anti-hoisting, skipping",
                );
                continue;
            };

            // If user has no nickname, use username
            let old_nickname = match &hoisted_member.nick {
                Some(name) => name,
                None => &hoisted_member.user.name,
            };

            // how is the name transformed
            let new_nickname = match config.change_name_using {
                ChangeNameUsing::Deunicode { ref mapping } => deunicode::deunicode(old_nickname)
                    .chars()
                    .map(|c| mapping.get(&c).copied().unwrap_or(c))
                    .collect::<String>(),
                ChangeNameUsing::Fixed { ref new_name } => new_name.to_string(),
            };

            // makes sure the new name is different than the old name
            // prevents false-triggers
            if *old_nickname == new_nickname {
                debug!(
                    name = %new_nickname,
                    help = "try adding the special character or trigger regex",
                    "New name is equal to old name"
                );
                continue;
            }

            let new_nickname = AntiHoisting::truncate_nickname(&new_nickname,twilight_validate::request::NICKNAME_LIMIT_MAX);

            let result = Self::change_nickname(hoisted_member, &new_nickname, &http).await;

            match result {
                Err(err) => {
                    // permission errors
                    debug!(error = %err, "Failed to change nickname");
                    continue;
                }
                Ok(m) if m.nick.as_ref().is_some_and(|nick| *nick == new_nickname) => {
                    debug!(
                        old_name = %old_nickname,
                        new_name = %&new_nickname,
                        "Member has tried to hoist",
                    );
                }
                Ok(_) => continue,
            };
        }
    }

    /// changes the nickname of a user to a new one
    async fn change_nickname(
        hoisted_member: &MemberUpdate,
        new_nickname: &str,
        http: &Arc<Client>,
    ) -> anyhow::Result<Member> {
        Ok(http
            .update_guild_member(hoisted_member.guild_id, hoisted_member.user.id)
            .nick(Some(new_nickname))
            .await?
            .model()
            .await?)
    }

    /// Defines what is considered a hoisted name
    fn is_hoisted(trigger: &Regex, nickname: &str) -> bool {
        trigger.is_match(nickname)
    }

    /// Truncates a nickname to a specified length
    pub fn truncate_nickname(nickname: &str, limit: usize) -> Cow<'_, str> {
        let cutoff = nickname
            .char_indices()
            .nth(limit)
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| nickname.len());

        if cutoff == nickname.len() {
            Cow::Borrowed(nickname)
        } else {
            Cow::Owned(nickname[..cutoff].to_string())
        }
    }
}
