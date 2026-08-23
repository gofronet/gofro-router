use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{fsops, paths};

const SCHEMA: u8 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpdateState {
    Idle,
    Checking,
    Available,
    Downloading,
    Installing,
    Success,
    Error,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct UpdateProgress {
    schema: u8,
    pub(crate) state: UpdateState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}

impl UpdateProgress {
    pub(crate) fn new(state: UpdateState, version: Option<&Version>) -> Self {
        Self {
            schema: SCHEMA,
            state,
            version: version.map(ToString::to_string),
            message: None,
        }
    }

    pub(crate) fn error(message: &str) -> Self {
        let message: String = message
            .chars()
            .filter(|character| !character.is_control())
            .take(500)
            .collect();
        Self {
            schema: SCHEMA,
            state: UpdateState::Error,
            version: None,
            message: Some(message),
        }
    }

    pub(crate) fn active(&self) -> bool {
        matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading | UpdateState::Installing
        )
    }

    fn validate(&self) -> Result<()> {
        ensure!(self.schema == SCHEMA, "unsupported update status schema");
        if let Some(version) = &self.version {
            let version = Version::parse(version).context("invalid update status version")?;
            ensure!(
                version.pre.is_empty() && version.build.is_empty(),
                "update status version is not stable"
            );
        }
        ensure!(
            self.message.as_ref().is_none_or(|message| {
                message.chars().count() <= 500 && !message.chars().any(char::is_control)
            }),
            "invalid update status message"
        );
        ensure!(
            !matches!(
                self.state,
                UpdateState::Available
                    | UpdateState::Downloading
                    | UpdateState::Installing
                    | UpdateState::Success
            ) || self.version.is_some(),
            "update status is missing its version"
        );
        ensure!(
            self.state == UpdateState::Error || self.message.is_none(),
            "only error status may contain a message"
        );
        Ok(())
    }
}

pub(crate) fn read() -> Result<UpdateProgress> {
    let bytes = match fs::read(paths::PROGRESS) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UpdateProgress::new(UpdateState::Idle, None));
        }
        Err(error) => return Err(error).context("failed to read update status"),
    };
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Result<UpdateProgress> {
    let progress: UpdateProgress =
        serde_json::from_slice(bytes).context("failed to parse update status")?;
    progress.validate()?;
    Ok(progress)
}

pub(crate) fn write(progress: &UpdateProgress) -> Result<()> {
    progress.validate()?;
    fs::create_dir_all(paths::UPDATE_DIR).context("failed to create update state directory")?;
    let mut bytes = serde_json::to_vec(progress).context("failed to serialize update status")?;
    bytes.push(b'\n');
    fsops::atomic_write(Path::new(paths::PROGRESS), &bytes, 0o644)
}

pub(crate) fn write_error(error: &anyhow::Error) -> Result<()> {
    write(&UpdateProgress::error(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_status_contract_round_trips() {
        for json in [
            r#"{"schema":1,"state":"idle"}"#,
            r#"{"schema":1,"state":"checking"}"#,
            r#"{"schema":1,"state":"available","version":"1.2.3"}"#,
            r#"{"schema":1,"state":"downloading","version":"1.2.3"}"#,
            r#"{"schema":1,"state":"installing","version":"1.2.3"}"#,
            r#"{"schema":1,"state":"success","version":"1.2.3"}"#,
            r#"{"schema":1,"state":"error","message":"failed"}"#,
        ] {
            let status = parse(json.as_bytes()).unwrap();
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::from_str::<serde_json::Value>(json).unwrap()
            );
        }
        assert!(parse(br#"{"schema":2,"state":"idle"}"#).is_err());
        let error = UpdateProgress::error(&"я".repeat(600));
        assert_eq!(error.message.unwrap().chars().count(), 500);
    }
}
