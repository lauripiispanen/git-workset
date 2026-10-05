use serde_json::{json, Map, Value};

/// Failures a caller can act on, each with its own exit code. Attached to an
/// `anyhow::Error` as the root cause; anything without one is a plain failure
/// (exit 1). `Display` is the one-line `message`: it never embeds git stderr
/// or config text, which go in `details` instead.
#[derive(Debug)]
pub enum Kind {
    /// No `.git-workset.toml` at the requested source.
    ConfigMissing { source: String, reason: String },
    /// Syntax error, unknown key, wrong type or unsupported `version`.
    ConfigInvalid {
        source: String,
        reason: String,
        parse_error: Option<String>,
    },
    /// A requested profile, or one part of `a+b`, does not exist.
    UnknownProfile {
        requested: String,
        available: Vec<String>,
    },
    /// An object that exists in the tree could not be read: it is not local
    /// and fetching it from the promisor remote failed.
    FetchFailed { source: String, git_stderr: String },
}

impl Kind {
    pub fn code(&self) -> &'static str {
        match self {
            Kind::ConfigMissing { .. } => "config_missing",
            Kind::ConfigInvalid { .. } => "config_invalid",
            Kind::UnknownProfile { .. } => "unknown_profile",
            Kind::FetchFailed { .. } => "fetch_failed",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Kind::ConfigMissing { .. } => 3,
            Kind::ConfigInvalid { .. } => 4,
            Kind::UnknownProfile { .. } => 5,
            Kind::FetchFailed { .. } => 6,
        }
    }

    fn details(&self) -> Value {
        match self {
            Kind::ConfigMissing { source, .. } => json!({ "source": source }),
            Kind::ConfigInvalid {
                source,
                parse_error,
                ..
            } => {
                let mut d = json!({ "source": source });
                if let Some(p) = parse_error {
                    d["parse_error"] = json!(p);
                }
                d
            }
            Kind::UnknownProfile {
                requested,
                available,
            } => json!({ "requested": requested, "available": available }),
            Kind::FetchFailed { source, git_stderr } => {
                json!({ "source": source, "git_stderr": git_stderr })
            }
        }
    }

    /// Multi-line detail worth showing a human after the message.
    fn extra(&self) -> Option<&str> {
        match self {
            Kind::ConfigInvalid { parse_error, .. } => parse_error.as_deref(),
            Kind::FetchFailed { git_stderr, .. } => Some(git_stderr),
            _ => None,
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::ConfigMissing { source, reason } => write!(f, "{}: {}", source, reason),
            Kind::ConfigInvalid { source, reason, .. } => {
                write!(f, "invalid config in {}: {}", source, reason)
            }
            Kind::UnknownProfile {
                requested,
                available,
            } if available.is_empty() => write!(
                f,
                "Workset '{}' not found: the config defines no worksets",
                requested
            ),
            Kind::UnknownProfile {
                requested,
                available,
            } => write!(
                f,
                "Workset '{}' not found. Available: {}",
                requested,
                available.join(", ")
            ),
            Kind::FetchFailed { source, .. } => write!(
                f,
                "could not read {}: it is not available locally and fetching it failed",
                source
            ),
        }
    }
}

impl std::error::Error for Kind {}

/// The `Kind` anywhere in the error's chain, if any.
pub fn kind_of(err: &anyhow::Error) -> Option<&Kind> {
    err.chain().find_map(|e| e.downcast_ref::<Kind>())
}

pub fn exit_code(err: &anyhow::Error) -> i32 {
    kind_of(err).map(Kind::exit_code).unwrap_or(1)
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or_default()
}

/// The text printed on stderr for a human.
pub fn human(err: &anyhow::Error) -> String {
    match kind_of(err) {
        Some(kind) => match kind.extra() {
            Some(extra) => format!("{}\n{}", kind, extra.trim_end()),
            None => kind.to_string(),
        },
        None => format!("{:#}", err),
    }
}

/// The `error@1` document printed on stdout under `--json`. `message` is a
/// single line; anything taken from git or the config file is under
/// `details` (`git_stderr`, `parse_error`, `cause`) and is untrusted.
pub fn to_json(err: &anyhow::Error) -> Value {
    let (code, message, details) = match kind_of(err) {
        Some(k) => (k.code(), k.to_string(), k.details()),
        None => {
            let mut d = Map::new();
            d.insert("cause".into(), json!(format!("{:#}", err)));
            ("failed", first_line(&err.to_string()).to_string(), d.into())
        }
    };
    json!({
        "schema": "git-workset/error@1",
        "code": code,
        "message": first_line(&message),
        "details": details,
    })
}
