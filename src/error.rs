use serde_json::{json, Value};

/// Failures a caller can act on, each with its own exit code. Attached to an
/// `anyhow::Error` as the root cause; anything without one is a plain failure
/// (exit 1).
#[derive(Debug)]
pub enum Kind {
    /// No `.git-workset.toml` at the requested source.
    ConfigMissing { source: String },
    /// Syntax error, unknown key, wrong type or unsupported `version`.
    ConfigInvalid { source: String },
    /// A requested profile, or one part of `a+b`, does not exist.
    UnknownProfile {
        requested: String,
        available: Vec<String>,
    },
}

impl Kind {
    pub fn code(&self) -> &'static str {
        match self {
            Kind::ConfigMissing { .. } => "config_missing",
            Kind::ConfigInvalid { .. } => "config_invalid",
            Kind::UnknownProfile { .. } => "unknown_profile",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Kind::ConfigMissing { .. } => 3,
            Kind::ConfigInvalid { .. } => 4,
            Kind::UnknownProfile { .. } => 5,
        }
    }

    fn details(&self) -> Value {
        match self {
            Kind::ConfigMissing { source } | Kind::ConfigInvalid { source } => {
                json!({ "source": source })
            }
            Kind::UnknownProfile {
                requested,
                available,
            } => json!({ "requested": requested, "available": available }),
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::ConfigMissing { source } => write!(f, "no .git-workset.toml at {}", source),
            Kind::ConfigInvalid { source } => write!(f, "invalid config in {}", source),
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

/// The `error@1` document printed on stdout under `--json`.
pub fn to_json(err: &anyhow::Error) -> Value {
    let (code, details) = match kind_of(err) {
        Some(k) => (k.code(), k.details()),
        None => ("failed", json!({})),
    };
    json!({
        "schema": "git-workset/error@1",
        "code": code,
        "message": format!("{:#}", err),
        "details": details,
    })
}
