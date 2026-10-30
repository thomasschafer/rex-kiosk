//! Talking to the Rex server through its CLI, `rex api call`, and the IDs and errors
//! that go with it.

use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

pub mod backend;
mod layout;
mod live;

pub use backend::Rex;

/// The block flavor for a terminal running the user's shell.
pub const SHELL_FLAVOR: &str = "com.superlogical.terminal.shell";

macro_rules! rex_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;

            fn try_from(id: String) -> Result<Self, String> {
                match id.strip_prefix(Self::PREFIX) {
                    Some(rest) if !rest.is_empty() => Ok(Self(id)),
                    _ => Err(format!(
                        "{id:?} is not a Rex ID starting with {:?}",
                        Self::PREFIX
                    )),
                }
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

rex_id!(
    /// A session: what Rex.app lists in its sidebar.
    SessionId,
    "session:"
);
rex_id!(
    /// A window: a tab in a session.
    WindowId,
    "window:"
);
rex_id!(
    /// A block: one pane, such as a terminal.
    BlockId,
    "block:"
);
rex_id!(
    /// A program connected to the server, such as Rex.app on one Mac.
    ClientId,
    "client:"
);

/// A window and the session it is in: what kiosk opens a checkout or folder in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowRef {
    pub session: SessionId,
    pub window: WindowId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RexError {
    /// The server refused a request, with its error code, such as `not_found`.
    Api {
        code: String,
        message: String,
    },
    /// Rex.app declined to switch session because Remote Control is off.
    RemoteControlOff,
    /// More than one session could be the repository's, so kiosk will not guess.
    AmbiguousSession {
        repo: String,
        labels: Vec<String>,
    },
    Git(String),
    InvalidArgument(String),
    Invocation(String),
    InvalidResponse(String),
}

impl RexError {
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code),
            _ => None,
        }
    }
}

impl fmt::Display for RexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api { code, message } => write!(formatter, "{message} ({code})"),
            Self::RemoteControlOff => formatter.write_str(
                "Rex.app is not accepting actions from kiosk, so it could not switch to the \
                 session: turn on Remote Control in Rex's Settings, under Rex Server, then \
                 open it again",
            ),
            Self::AmbiguousSession { repo, labels } => write!(
                formatter,
                "{repo} has windows in more than one Rex session ({}), so kiosk cannot tell \
                 which to add to; label exactly one of them {repo:?}",
                labels.join(", ")
            ),
            Self::Git(message) => formatter.write_str(message),
            Self::InvalidArgument(message) => write!(formatter, "invalid argument: {message}"),
            Self::Invocation(message) => write!(formatter, "rex invocation failed: {message}"),
            Self::InvalidResponse(message) => write!(formatter, "invalid rex response: {message}"),
        }
    }
}

impl std::error::Error for RexError {}

/// The Rex server's control API, as kiosk uses it.
pub trait RexApi: Send + Sync {
    /// Calls `method` with `args` and returns its result.
    fn call(&self, method: &str, args: &Value) -> Result<Value, RexError>;
    /// Asks `client` to perform one of its own actions, such as showing a session.
    fn perform(&self, client: &ClientId, action: &str, args: &Value) -> Result<(), RexError>;
}

/// Calls methods with a `rex` binary: `rex api call METHOD ARGS`, and performs client
/// actions with `rex -C CLIENT do ACTION`, which explains a refusal in terms of the app's
/// settings.
#[derive(Debug, Clone)]
pub struct CliRex {
    binary: PathBuf,
}

impl CliRex {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }
}

impl CliRex {
    /// Runs `rex` with `args` and returns its standard output. `what` names the request
    /// in errors.
    fn run(&self, what: &str, args: &[&str]) -> Result<Vec<u8>, RexError> {
        let output = Command::new(&self.binary)
            .args(args)
            .output()
            .map_err(|error| {
                RexError::Invocation(format!(
                    "could not execute {}: {error}",
                    self.binary.display()
                ))
            })?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(parse_failure(&stderr).unwrap_or_else(|| {
            RexError::Invocation(format!(
                "{} {what} exited with {}: {}",
                self.binary.display(),
                output.status,
                stderr.trim()
            ))
        }))
    }
}

impl RexApi for CliRex {
    fn call(&self, method: &str, args: &Value) -> Result<Value, RexError> {
        let stdout = self.run(
            &format!("api call {method}"),
            &["api", "call", method, &args.to_string()],
        )?;
        serde_json::from_slice(&stdout).map_err(|error| {
            RexError::InvalidResponse(format!(
                "{method}: {error}; stdout was {:?}",
                String::from_utf8_lossy(&stdout).trim()
            ))
        })
    }

    fn perform(&self, client: &ClientId, action: &str, args: &Value) -> Result<(), RexError> {
        self.run(
            &format!("do {action}"),
            &[
                "-C",
                client.as_str(),
                "do",
                action,
                "--args",
                &args.to_string(),
            ],
        )
        .map(drop)
    }
}

/// Reads a refusal from `rex`'s standard error, which ends its first line with the
/// server's status and code: `Error: ...: session X not found (status=404
/// code=session_not_found)`.
fn parse_failure(stderr: &str) -> Option<RexError> {
    let line = stderr.lines().next()?.trim();
    let line = line.strip_prefix("Error: ").unwrap_or(line);
    let (message, details) = line.strip_suffix(')')?.rsplit_once(" (status=")?;
    let (_, code) = details.split_once(" code=")?;
    if code.is_empty() || code.contains(char::is_whitespace) {
        return None;
    }
    // The CLI prefixes the server's message with the request that failed.
    let message = message
        .split_once(" failed: ")
        .filter(|(request, _)| request.starts_with("stream response "))
        .map_or(message, |(_, message)| message);
    Some(RexError::Api {
        code: code.into(),
        message: message.into(),
    })
}

/// Deserializes a method's result.
pub(crate) fn decode<T: DeserializeOwned>(method: &str, value: Value) -> Result<T, RexError> {
    serde_json::from_value(value)
        .map_err(|error| RexError::InvalidResponse(format!("{method}: {error}")))
}

pub(crate) fn path_arg(name: &str, path: &Path) -> Result<String, RexError> {
    if !path.is_absolute() {
        return Err(RexError::InvalidArgument(format!(
            "{name} must be absolute: {}",
            path.display()
        )));
    }
    path.to_str().map(str::to_owned).ok_or_else(|| {
        RexError::InvalidArgument(format!(
            "{name} is not valid UTF-8 and cannot be passed to Rex: {}",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_must_carry_their_kind() {
        assert_eq!(
            SessionId::try_from("session:01a1".to_string())
                .unwrap()
                .as_str(),
            "session:01a1"
        );
        for bad in ["window:01a1", "session:", "01a1", ""] {
            let error = SessionId::try_from(bad.to_string()).unwrap_err();
            assert!(error.contains("session:"), "{error}");
        }
        let parsed: Result<BlockId, _> = serde_json::from_value(Value::from("window:1"));
        assert!(parsed.is_err());
    }

    #[test]
    fn refusals_keep_the_servers_code_and_message() {
        let error = parse_failure(
            "Error: stream response \"control-1\" failed: session session:01 not found \
             (status=404 code=session_not_found)\n",
        )
        .unwrap();
        assert_eq!(
            error,
            RexError::Api {
                code: "session_not_found".into(),
                message: "session session:01 not found".into(),
            }
        );
        let refused =
            parse_failure("Error: Rex.app is not accepting actions. (status=403 code=refused)")
                .unwrap();
        assert_eq!(refused.code(), Some("refused"));
    }

    #[test]
    fn other_failures_are_not_mistaken_for_refusals() {
        for stderr in [
            "Error: invalid session_id: invalid UUID length: 4\n\nUsage:\n  rex api call",
            "",
            "Error: odd (status=500 code=)",
        ] {
            assert_eq!(parse_failure(stderr), None, "{stderr:?}");
        }
    }

    #[test]
    fn paths_must_be_absolute() {
        assert!(path_arg("cwd", Path::new("relative")).is_err());
        assert_eq!(path_arg("cwd", Path::new("/repo")).unwrap(), "/repo");
    }

    #[cfg(unix)]
    mod cli {
        use std::{fs, os::unix::fs::PermissionsExt};

        use super::*;

        fn fake_rex(body: &str) -> (tempfile::TempDir, CliRex) {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("rex");
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            (temp, CliRex::new(path))
        }

        #[test]
        fn calls_pass_the_method_and_json_args_as_words() {
            let (temp, rex) =
                fake_rex(r#"printf '%s\n' "$@" > "$(dirname "$0")/argv"; echo '{"ok":true}'"#);

            let result = rex
                .call(
                    "session.view",
                    &serde_json::json!({ "session_id": "session:1" }),
                )
                .unwrap();

            assert_eq!(result, serde_json::json!({ "ok": true }));
            assert_eq!(
                fs::read_to_string(temp.path().join("argv")).unwrap(),
                "api\ncall\nsession.view\n{\"session_id\":\"session:1\"}\n"
            );
        }

        #[test]
        fn client_actions_name_the_client_and_pass_json_args() {
            let (temp, rex) = fake_rex(r#"printf '%s\n' "$@" > "$(dirname "$0")/argv""#);

            rex.perform(
                &ClientId::try_from("client:7".to_string()).unwrap(),
                "session.select",
                &serde_json::json!({ "session_id": "session:1" }),
            )
            .unwrap();

            assert_eq!(
                fs::read_to_string(temp.path().join("argv")).unwrap(),
                "-C\nclient:7\ndo\nsession.select\n--args\n{\"session_id\":\"session:1\"}\n"
            );
        }

        #[test]
        fn failures_report_the_servers_refusal_or_the_exit() {
            let (_temp, refusing) = fake_rex(
                "echo 'Error: block block:9 not found (status=404 code=not_found)' >&2; exit 3",
            );
            assert_eq!(
                refusing.call("block.close", &Value::Null).unwrap_err(),
                RexError::Api {
                    code: "not_found".into(),
                    message: "block block:9 not found".into(),
                }
            );

            let (_temp, crashing) = fake_rex("echo 'no server' >&2; exit 1");
            let error = crashing.call("session.list", &Value::Null).unwrap_err();
            assert!(
                matches!(&error, RexError::Invocation(message) if message.contains("no server")),
                "{error}"
            );

            let (_temp, garbled) = fake_rex("echo not json");
            assert!(matches!(
                garbled.call("session.list", &Value::Null).unwrap_err(),
                RexError::InvalidResponse(_)
            ));
        }
    }
}
