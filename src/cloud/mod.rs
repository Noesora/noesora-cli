use std::process::ExitCode;

use clap::Subcommand;
use noesora_engine::Store;
use serde_json::{json, Value};

mod api;
mod auth;
mod local;
mod sync;

#[derive(Subcommand)]
pub(crate) enum CloudCommand {
    /// Sign in with the configured Cloud public client.
    Login {
        /// HTTPS API origin (HTTP is accepted only for an explicitly selected loopback fixture).
        #[arg(long)]
        url: String,
    },
    /// Revoke the current tokens when supported and remove them from OS secure storage.
    Logout,
    /// Show the current authenticated Cloud identity and organization memberships.
    Status,
    /// Review exact selected objects, then confirm disclosure with the preview SHA-256.
    Push {
        #[arg(long)]
        org: String,
        #[arg(long = "record")]
        records: Vec<String>,
        #[arg(long = "session")]
        sessions: Vec<String>,
        /// Exact preview digest from the previous invocation.
        #[arg(long)]
        confirm: Option<String>,
    },
    /// Validate and apply remote changes for one organization.
    Pull {
        #[arg(long)]
        org: String,
    },
}

#[derive(Debug)]
pub(crate) struct CloudError {
    pub(crate) category: String,
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) http_status: Option<u16>,
    pub(crate) request_id: Option<String>,
    pub(crate) details: Option<Value>,
}

impl CloudError {
    pub(crate) fn new(category: &str, code: &str, message: impl Into<String>) -> Self {
        Self {
            category: category.to_owned(),
            code: code.to_owned(),
            message: message.into(),
            http_status: None,
            request_id: None,
            details: None,
        }
    }

    pub(crate) fn local_store(message: impl Into<String>) -> Self {
        Self::new("local", "LOCAL_STORE_FAILED", message)
    }

    pub(crate) fn integrity(message: impl Into<String>) -> Self {
        Self::new("integrity", "CLOUD_INTEGRITY_REFUSAL", message)
    }

    pub(crate) fn offline(message: impl Into<String>) -> Self {
        Self::new("offline", "CLOUD_OFFLINE", message)
    }

    pub(crate) fn confirmation(message: impl Into<String>) -> Self {
        Self::new("consent", "CLOUD_CONFIRMATION_REQUIRED", message)
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new("conflict", "CLOUD_CONFLICT", message)
    }

    pub(crate) fn with_http(mut self, status: u16, request_id: Option<String>) -> Self {
        self.http_status = Some(status);
        self.request_id = request_id;
        self
    }

    pub(crate) fn json(&self) -> Value {
        let mut value = json!({
            "ok": false,
            "category": self.category,
            "code": self.code,
            "error": self.message,
        });
        if let Some(status) = self.http_status {
            value["http_status"] = json!(status);
        }
        if let Some(request_id) = &self.request_id {
            value["request_id"] = json!(request_id);
        }
        if let Some(details) = &self.details {
            value["details"] = details.clone();
        }
        value
    }
}

#[derive(Debug)]
pub(crate) struct CloudOutput {
    pub(crate) payload: Value,
    pub(crate) text: String,
    pub(crate) exit: ExitCode,
}

impl CloudOutput {
    pub(crate) fn success(command: &str, payload: Value, text: impl Into<String>) -> Self {
        let mut value = payload;
        if !value.is_object() {
            value = json!({"result": value});
        }
        value["ok"] = json!(true);
        value["command"] = json!(command);
        Self {
            payload: value,
            text: text.into(),
            exit: ExitCode::SUCCESS,
        }
    }
}

pub(crate) fn execute(command: CloudCommand, store: &Store) -> Result<CloudOutput, CloudError> {
    match command {
        CloudCommand::Login { url } => auth::login(store, &url),
        CloudCommand::Logout => auth::logout(store),
        CloudCommand::Status => auth::status(store),
        CloudCommand::Push {
            org,
            records,
            sessions,
            confirm,
        } => sync::push(store, &org, &records, &sessions, confirm.as_deref()),
        CloudCommand::Pull { org } => sync::pull(store, &org),
    }
}

pub(crate) fn clean_terminal(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\n' | '\t' => ch,
            ch if ch.is_control() => '�',
            ch => ch,
        })
        .collect()
}

pub(crate) fn emit_error(json_output: bool, error: CloudError) {
    let message = clean_terminal(&error.message);
    if json_output {
        println!("{}", error.json());
    } else {
        println!("noesora: {message}");
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) use super::auth::test_support::*;
    pub(crate) use super::sync::test_support::*;
}
