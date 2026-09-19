//! Computer / session notices — the production path to a push credential
//! boundary.
//!
//! Desktop always records a [`ComputerNotice`]. Remote clients consume them
//! through `WatchComputerEvents`. Delivery to a phone is a separate sink:
//! when FCM/APNs credentials are missing the sink reports
//! `credential_missing` instead of pretending a push succeeded.

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoticeKind {
    TaskComplete,
    ApprovalRequired,
    ComputerForeground,
    Failure,
    Offline,
    Reconnect,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerNotice {
    pub kind: NoticeKind,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub at: chrono::DateTime<chrono::Utc>,
    /// How far a push actually got. `local_only` is not a fake success.
    pub delivery: PushDelivery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PushDelivery {
    /// Recorded on the desktop event bus only.
    LocalOnly,
    /// A push provider was configured and accepted the message.
    Pushed,
    /// Production path reached the credential boundary — no FCM/APNs key.
    CredentialMissing { provider: String, reason: String },
}

impl PushDelivery {
    pub fn from_env() -> Self {
        match std::env::var("COSMOS_PUSH_PROVIDER") {
            Ok(provider) if !provider.trim().is_empty() => {
                if std::env::var("COSMOS_PUSH_CREDENTIAL")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .is_some()
                {
                    // Credential present is not the same as a successful send.
                    // The engine still has to actually POST; until a real
                    // provider client is wired we stay honest.
                    Self::CredentialMissing {
                        provider,
                        reason: "push provider selected but no send client is wired in this build"
                            .into(),
                    }
                } else {
                    Self::CredentialMissing {
                        provider,
                        reason: "COSMOS_PUSH_CREDENTIAL is not set".into(),
                    }
                }
            }
            _ => Self::LocalOnly,
        }
    }
}

pub fn notice(
    kind: NoticeKind,
    title: impl Into<String>,
    body: impl Into<String>,
) -> ComputerNotice {
    ComputerNotice {
        kind,
        title: title.into(),
        body: body.into(),
        chat_id: None,
        device_id: None,
        at: chrono::Utc::now(),
        delivery: PushDelivery::from_env(),
    }
}

/// Fan-out of notices. `watch` keeps the latest; subscribers also get it as
/// the first `WatchComputerEvents` item.
pub fn channel() -> (
    watch::Sender<Option<ComputerNotice>>,
    watch::Receiver<Option<ComputerNotice>>,
) {
    watch::channel(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_credentials_are_not_success() {
        // Safety: tests run single-threaded here for this env.
        let previous = std::env::var("COSMOS_PUSH_PROVIDER").ok();
        unsafe { std::env::remove_var("COSMOS_PUSH_PROVIDER") };
        let delivery = PushDelivery::from_env();
        assert!(matches!(delivery, PushDelivery::LocalOnly));
        unsafe { std::env::set_var("COSMOS_PUSH_PROVIDER", "fcm") };
        unsafe { std::env::remove_var("COSMOS_PUSH_CREDENTIAL") };
        let delivery = PushDelivery::from_env();
        assert!(matches!(
            delivery,
            PushDelivery::CredentialMissing { ref provider, .. } if provider == "fcm"
        ));
        match previous {
            Some(v) => unsafe { std::env::set_var("COSMOS_PUSH_PROVIDER", v) },
            None => unsafe { std::env::remove_var("COSMOS_PUSH_PROVIDER") },
        }
    }
}
