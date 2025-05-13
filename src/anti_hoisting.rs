use std::sync::Arc;

use regex::Regex;
use tokio::sync::broadcast::{Receiver, error::RecvError};
use tracing::debug;
use twilight_gateway::Event;
use twilight_http::Client;
use twilight_model::{
    gateway::{Intents, payload::incoming::MemberUpdate},
    guild::Member,
    id::{Id, marker::RoleMarker},
};

// https://discord.com/developers/docs/resources/user
fn default_max_nickname_length() -> u32 {
    32
}

fn default_min_nickname_length() -> u32 {
    1
}

use serde::de::{Deserialize, Deserializer, Error};
fn deserialize_regex<'de, D>(deserializer: D) -> Result<Regex, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Regex::new(&s).map_err(Error::custom)
}

#[derive(Debug)]
pub struct Configuration {
    /// The maximum length of a nickname
    /// defaults to 32
    /// ONLY CHANGE THIS IF YOU KNOW WHAT YOU ARE DOING
    pub max_nickname_length: u32,
    /// The minimum length of a nickname
    /// defaults to 1
    /// ONLY CHANGE THIS IF YOU KNOW WHAT YOU ARE DOING
    pub min_nickname_length: u32,
    /// Name format that triggers anti-hoisting
    /// e.g. ^[A-Za-z]{2,}
    pub trigger: Regex, // TODO only single letters are allowed
    /// The output message to send to the user
    /// Must contain the literal `${nickname}`
    /// Example: `Box<${nickname}>`
    pub output: String,
    /// Roles that are able to bypass anti-hoisting
    pub ignore_roles: Vec<Id<RoleMarker>>
}

/// Deserialize Configuration
/// This is needed to provide config error handling at the time of deserialization
impl<'de> Deserialize<'de> for Configuration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de> {

        #[derive(serde::Deserialize)]
        struct RawConfiguration {
            #[serde(default = "default_max_nickname_length")]
            max_nickname_length: u32,
            #[serde(default = "default_min_nickname_length")]
            min_nickname_length: u32,
            #[serde(deserialize_with = "deserialize_regex")]
            trigger: Regex,
            output: String,
            #[serde(default)]
            ignore_roles: Vec<Id<RoleMarker>>
        }

        let RawConfiguration {
            max_nickname_length,
            min_nickname_length,
            trigger,
            output,
            ignore_roles
        } = RawConfiguration::deserialize(deserializer)?;

        let output = output.trim();
        
        // invalid if output does not contain ${nickname}
        if !output.contains(AntiHoisting::NICKNAME_PLACEHOLDER) {
            return Err(Error::custom("`output` must contain `${nickname}`"));
        };

        //TODO validation

        Ok(Configuration {
            max_nickname_length,
            min_nickname_length,
            trigger,
            output: output.to_owned(),
            ignore_roles,
        })
    }
}

pub struct AntiHoisting {}

impl AntiHoisting {
    pub const INTENTS: Intents = Intents::GUILD_MEMBERS;
    pub const NICKNAME_PLACEHOLDER: &str = "${nickname}";

    /// run the main anti-hoisting logic
    pub async fn serve(config: Configuration, mut events: Receiver<Arc<Event>>, http: Arc<Client>) {
        loop {
            let event = events.recv().await;

            // match events where a member's name is hoisted
            let hoisted_member = match event.as_deref() {
                Err(RecvError::Closed) => return,
                Err(_) => continue,
                Ok(Event::MemberUpdate(m)) if m.nick.as_ref().map_or(false, |nick| AntiHoisting::is_hoisted(&config.trigger, &nick))  => m,
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
            
            // Only nicknames are supported
            let Some(ref old_nickname) = hoisted_member.nick else {
                continue;
            };

            let new_nickname = AntiHoisting::transform(&old_nickname, &config.output);

            // the output can produce a nickname longer than max_nickname_length
            // for now the strategy is to not change the nickname
            if new_nickname.len() > config.max_nickname_length as usize {
                continue;
            }

            let result = Self::change_nickname(&hoisted_member, &new_nickname, &http).await;

            let member= match result {
                Err(err) => {
                    // permission errors
                    debug!(error = %err, "Failed to change nickname");
                    continue;
                },
                Ok(m) if m.nick.as_ref().map_or(false, |nick| *nick == new_nickname) => m,
                Ok(_) => continue,
            };

            debug!(
                old_name = %hoisted_member.user.name,
                new_name = %&new_nickname,
                "Member has tried to hoist",
            );
        }
    }

    async fn change_nickname(hoisted_member: &Box<MemberUpdate>, new_nickname: &str, http: &Arc<Client>) -> anyhow::Result<Member> {
        Ok(http
            .update_guild_member(hoisted_member.guild_id, hoisted_member.user.id)
            .nick(Some(&new_nickname))
            .await?.model().await?)
    }   

    fn is_hoisted(trigger: &Regex, nickname: &str) -> bool {
        trigger.is_match(nickname)
    }

    fn transform(old_nickname: &str, output: &str) -> String {
        output.replace(Self::NICKNAME_PLACEHOLDER,old_nickname)
    }
}

