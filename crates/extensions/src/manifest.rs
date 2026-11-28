//! The extension manifest: one JSON document describing an extension and
//! carrying its payload inline (ADR 0012, #265 §1–3).
//!
//! [`Manifest::parse`] checks every rule before anything is stored or run.
//! Each error names the field it is about, as a path such as
//! `settings[2].key` or `permissions.hosts[0]`.

use std::cmp::Ordering;

use domain::ExtensionId;
use net::DeclaredHost;
use semver::Version;
use serde_json::{Map, Value};
use url::Url;

use crate::host::SUPPORTED_API_VERSIONS;

/// The largest manifest without its payload, in bytes of compact JSON.
pub const MAX_DESCRIPTOR_BYTES: usize = 64 * 1024;
/// The largest payload, in bytes: UTF-8 Luau source, or the declarative
/// definition as compact JSON.
pub const MAX_PAYLOAD_BYTES: usize = 512 * 1024;
/// The shortest and longest `id`, in characters.
const ID_LENGTH: std::ops::RangeInclusive<usize> = 3..=64;
/// The longest `name` and `author`, in characters.
const MAX_NAME_CHARS: usize = 64;

/// The fields that hold the payload, left out of the descriptor size.
const PAYLOAD_FIELDS: [&str; 2] = ["payload", "declarative"];

/// A validated extension manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// The slug: 3–64 characters of `a-z`, `0-9` and `-`, not starting or
    /// ending with `-`. Unique among installed extensions.
    pub id: ExtensionId,
    /// The display name, 1–64 characters.
    pub name: String,
    /// Who made it, 1–64 characters.
    pub author: String,
    /// A strict semantic version. Updates compare by precedence, ignoring `+build`.
    pub version: Version,
    /// The host API and provider contract version; one of
    /// [`SUPPORTED_API_VERSIONS`].
    pub api_version: u32,
    /// What the extension provides.
    pub kind: ExtensionKind,
    /// The code or definition to run; its variant is the manifest's
    /// `runtime`.
    pub payload: Payload,
    /// Where the payload was fetched from, when it was (HTTPS).
    pub payload_url: Option<Url>,
    /// The hosts the extension may contact (`permissions.hosts`).
    pub hosts: Vec<DeclaredHost>,
    /// A short description.
    pub description: Option<String>,
    /// An HTTPS icon URL.
    pub icon: Option<Url>,
    /// An HTTPS website URL.
    pub website: Option<Url>,
    /// A license name or SPDX expression.
    pub license: Option<String>,
    /// The content language tag, such as `en`.
    pub lang: Option<String>,
    /// Free-form notes for users.
    pub notes: Option<String>,
    /// The oldest Autumn version the extension runs on.
    pub min_app_version: Option<Version>,
    /// The payload's SHA-256, as 64 lowercase hex characters.
    pub sha256: Option<String>,
    /// The extension's settings schema, in order.
    pub settings: Vec<SettingField>,
}

/// What an extension provides. Phase 4 has only torrent providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtensionKind {
    /// `torrent-provider`: searches like an indexer.
    TorrentProvider,
}

impl ExtensionKind {
    /// The manifest spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TorrentProvider => "torrent-provider",
        }
    }
}

/// The extension's code or definition, inlined.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Payload {
    /// `runtime: "luau"`: the Luau source from `payload`.
    Luau(String),
    /// `runtime: "declarative"`: the definition object from `declarative`,
    /// run by the declarative runtime (ADR 0016).
    Declarative(Map<String, Value>),
}

impl Payload {
    /// The manifest's `runtime` spelling.
    #[must_use]
    pub const fn runtime(&self) -> &'static str {
        match self {
            Self::Luau(_) => "luau",
            Self::Declarative(_) => "declarative",
        }
    }

    /// The payload as stored: Luau source, or the definition as compact JSON.
    #[must_use]
    pub fn to_stored(&self) -> String {
        match self {
            Self::Luau(source) => source.clone(),
            Self::Declarative(definition) => Value::Object(definition.clone()).to_string(),
        }
    }
}

/// One setting an extension declares (#265 §3). Values are stored by the
/// settings ticket.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingField {
    /// `a-z`, `0-9` and `_`; unique in the manifest.
    pub key: String,
    /// The label shown in the settings form; not empty.
    pub label: String,
    /// Help text under the field.
    pub description: Option<String>,
    /// Whether the extension can't run until it is set. `false` by default.
    pub required: bool,
    /// The field's type and its type-specific rules.
    pub kind: SettingKind,
}

/// A setting's type.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SettingKind {
    /// `text`: a string.
    Text {
        /// The value when unset.
        default: Option<String>,
    },
    /// `number`: a number within optional bounds.
    Number {
        /// The smallest allowed value.
        min: Option<f64>,
        /// The largest allowed value.
        max: Option<f64>,
        /// Whether the value must be a whole number. `false` by default.
        integer: bool,
        /// The value when unset; within the bounds.
        default: Option<f64>,
    },
    /// `toggle`: a boolean.
    Toggle {
        /// The value when unset.
        default: Option<bool>,
    },
    /// `select`: one of a fixed list of strings.
    Select {
        /// The choices, at least one, with unique values.
        options: Vec<SelectOption>,
        /// The value when unset; one of the option values.
        default: Option<String>,
    },
    /// `secret`: a string kept only in the OS keychain. Has no default.
    Secret,
}

/// One choice of a `select` setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    /// The stored value.
    pub value: String,
    /// What the form shows.
    pub label: String,
}

/// Why a manifest was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ManifestError {
    /// The text is not JSON, or not a JSON object.
    #[error("the manifest is not a JSON object: {0}")]
    Json(String),
    /// The manifest without its payload, or the payload, is over its cap.
    #[error("{part} is {size} bytes, over the {limit}-byte limit")]
    TooLarge {
        /// `manifest` (without the payload) or `payload`.
        part: &'static str,
        /// Its size in bytes.
        size: usize,
        /// The cap in bytes.
        limit: usize,
    },
    /// The extension targets a host API version this app doesn't run.
    #[error(
        "api_version {found} is not supported; this version of Autumn runs api_version {}",
        supported_list()
    )]
    UnsupportedApiVersion {
        /// The manifest's `api_version`.
        found: u64,
    },
    /// The extension needs a newer Autumn.
    #[error("the extension needs Autumn {required} or newer; this is {current}")]
    AppTooOld {
        /// The manifest's `min_app_version`.
        required: Version,
        /// This app's version.
        current: Version,
    },
    /// The payload is only a `payload_url`; it must be fetched and inlined
    /// before install.
    #[error("payload: the manifest has only a payload_url; fetch it and inline it as {0} first")]
    PayloadNotFetched(&'static str),
    /// A field is missing or breaks its rule.
    #[error("{field}: {problem}")]
    Invalid {
        /// The field's path, such as `settings[2].key`.
        field: String,
        /// What is wrong, in plain words.
        problem: String,
    },
}

fn supported_list() -> String {
    SUPPORTED_API_VERSIONS
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn invalid(field: impl Into<String>, problem: impl Into<String>) -> ManifestError {
    ManifestError::Invalid {
        field: field.into(),
        problem: problem.into(),
    }
}

impl Manifest {
    /// Parses and validates a manifest with its payload inline.
    ///
    /// The checks run in this order: JSON shape, descriptor size,
    /// `api_version` and `min_app_version` (so an incompatible extension is
    /// reported as such), then every field.
    ///
    /// # Errors
    ///
    /// The first [`ManifestError`] found.
    pub fn parse(json: &str) -> Result<Self, ManifestError> {
        let value: Value =
            serde_json::from_str(json).map_err(|error| ManifestError::Json(error.to_string()))?;
        let Value::Object(object) = value else {
            return Err(ManifestError::Json(format!(
                "expected an object, got {}",
                json_type(&value)
            )));
        };
        Self::from_object(object)
    }

    /// Rebuilds a manifest stored as its descriptor plus its payload, as
    /// [`Self::descriptor`] and [`Payload::to_stored`] wrote them.
    ///
    /// # Errors
    ///
    /// As [`Self::parse`]: a stored manifest is checked again, so one this
    /// version no longer supports is refused.
    pub fn from_stored(descriptor: &str, payload: &str) -> Result<Self, ManifestError> {
        let value: Value = serde_json::from_str(descriptor)
            .map_err(|error| ManifestError::Json(error.to_string()))?;
        let Value::Object(mut object) = value else {
            return Err(ManifestError::Json("expected an object".to_owned()));
        };
        let payload = match object.get("runtime").and_then(Value::as_str) {
            Some("declarative") => {
                let definition = serde_json::from_str(payload)
                    .map_err(|error| ManifestError::Json(format!("declarative: {error}")))?;
                ("declarative", definition)
            }
            _ => ("payload", Value::String(payload.to_owned())),
        };
        object.insert(payload.0.to_owned(), payload.1);
        Self::from_object(object)
    }

    /// The manifest without its payload, as compact JSON, for storage.
    #[must_use]
    pub fn descriptor(&self) -> String {
        Value::Object(self.descriptor_object()).to_string()
    }

    /// Compares this manifest's version with `other` by semantic-version precedence,
    /// ignoring build metadata (`1.0.0+a` equals `1.0.0+b`).
    #[must_use]
    pub fn cmp_version(&self, other: &Version) -> Ordering {
        self.version.cmp_precedence(other)
    }

    fn from_object(mut object: Map<String, Value>) -> Result<Self, ManifestError> {
        let descriptor_size = {
            let mut descriptor = object.clone();
            for field in PAYLOAD_FIELDS {
                descriptor.remove(field);
            }
            Value::Object(descriptor).to_string().len()
        };
        if descriptor_size > MAX_DESCRIPTOR_BYTES {
            return Err(ManifestError::TooLarge {
                part: "manifest",
                size: descriptor_size,
                limit: MAX_DESCRIPTOR_BYTES,
            });
        }

        let api_version = api_version(&object)?;
        let min_app_version = optional_version(&object, "min_app_version")?;
        if let Some(required) = &min_app_version {
            let current = app_version();
            if required.cmp_precedence(&current) == Ordering::Greater {
                return Err(ManifestError::AppTooOld {
                    required: required.clone(),
                    current,
                });
            }
        }

        let id = slug(&object)?;
        let name = short_text(&object, "name")?;
        let author = short_text(&object, "author")?;
        let version_text = required_str(&object, "version")?;
        let version = Version::parse(version_text)
            .map_err(|error| invalid("version", format!("not a SemVer version: {error}")))?;
        let kind = match required_str(&object, "type")? {
            "torrent-provider" => ExtensionKind::TorrentProvider,
            other => {
                return Err(invalid(
                    "type",
                    format!("{other:?} is not supported; expected \"torrent-provider\""),
                ));
            }
        };
        let payload_url = optional_https(&object, "payload_url")?;
        let payload = payload(&mut object)?;
        let hosts = hosts(&object)?;

        Ok(Self {
            id,
            name,
            author,
            version,
            api_version,
            kind,
            payload,
            payload_url,
            hosts,
            description: optional_str(&object, "description")?,
            icon: optional_https(&object, "icon")?,
            website: optional_https(&object, "website")?,
            license: optional_str(&object, "license")?,
            lang: optional_str(&object, "lang")?,
            notes: optional_str(&object, "notes")?,
            min_app_version,
            sha256: sha256(&object)?,
            settings: settings(&object)?,
        })
    }

    fn descriptor_object(&self) -> Map<String, Value> {
        let mut object = Map::new();
        let mut put = |key: &str, value: Value| {
            object.insert(key.to_owned(), value);
        };
        put("id", self.id.as_str().into());
        put("name", self.name.as_str().into());
        put("author", self.author.as_str().into());
        put("version", self.version.to_string().into());
        put("api_version", self.api_version.into());
        put("type", self.kind.as_str().into());
        put("runtime", self.payload.runtime().into());
        let hosts: Vec<Value> = self
            .hosts
            .iter()
            .map(|host| host.to_string().into())
            .collect();
        put("permissions", serde_json::json!({ "hosts": hosts }));
        let optional = [
            ("payload_url", self.payload_url.as_ref().map(Url::to_string)),
            ("description", self.description.clone()),
            ("icon", self.icon.as_ref().map(Url::to_string)),
            ("website", self.website.as_ref().map(Url::to_string)),
            ("license", self.license.clone()),
            ("lang", self.lang.clone()),
            ("notes", self.notes.clone()),
            (
                "min_app_version",
                self.min_app_version.as_ref().map(Version::to_string),
            ),
            ("sha256", self.sha256.clone()),
        ];
        for (key, value) in optional {
            if let Some(value) = value {
                put(key, value.into());
            }
        }
        if !self.settings.is_empty() {
            put(
                "settings",
                self.settings.iter().map(SettingField::to_json).collect(),
            );
        }
        object
    }
}

impl SettingField {
    fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("key".to_owned(), self.key.as_str().into());
        object.insert("label".to_owned(), self.label.as_str().into());
        if let Some(description) = &self.description {
            object.insert("description".to_owned(), description.as_str().into());
        }
        if self.required {
            object.insert("required".to_owned(), true.into());
        }
        let (kind, extra): (&str, Vec<(&str, Option<Value>)>) = match &self.kind {
            SettingKind::Text { default } => ("text", vec![("default", default.clone().map(Value::from))]),
            SettingKind::Number { min, max, integer, default } => (
                "number",
                vec![
                    ("min", min.map(Value::from)),
                    ("max", max.map(Value::from)),
                    ("integer", integer.then_some(Value::Bool(true))),
                    ("default", default.map(Value::from)),
                ],
            ),
            SettingKind::Toggle { default } => ("toggle", vec![("default", default.map(Value::from))]),
            SettingKind::Select { options, default } => (
                "select",
                vec![
                    (
                        "options",
                        Some(
                            options
                                .iter()
                                .map(|option| {
                                    serde_json::json!({ "value": option.value, "label": option.label })
                                })
                                .collect(),
                        ),
                    ),
                    ("default", default.clone().map(Value::from)),
                ],
            ),
            SettingKind::Secret => ("secret", vec![]),
        };
        object.insert("type".to_owned(), kind.into());
        for (key, value) in extra {
            if let Some(value) = value {
                object.insert(key.to_owned(), value);
            }
        }
        Value::Object(object)
    }
}

/// This app's version: the workspace version every crate shares.
fn app_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0))
}

fn api_version(object: &Map<String, Value>) -> Result<u32, ManifestError> {
    let value = object
        .get("api_version")
        .ok_or_else(|| invalid("api_version", "missing; Phase 4 extensions use 1"))?;
    let found = value.as_u64().ok_or_else(|| {
        invalid(
            "api_version",
            format!("expected a whole number, got {}", describe(value)),
        )
    })?;
    u32::try_from(found)
        .ok()
        .filter(|version| SUPPORTED_API_VERSIONS.contains(version))
        .ok_or(ManifestError::UnsupportedApiVersion { found })
}

fn slug(object: &Map<String, Value>) -> Result<ExtensionId, ManifestError> {
    let id = required_str(object, "id")?;
    let chars_ok = id
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    let ends_ok = !id.starts_with('-') && !id.ends_with('-');
    if !ID_LENGTH.contains(&id.len()) || !chars_ok || !ends_ok {
        return Err(invalid(
            "id",
            format!(
                "{id:?} must be 3–64 characters of a-z, 0-9 and -, not starting or ending with -"
            ),
        ));
    }
    Ok(ExtensionId::new(id))
}

fn short_text(object: &Map<String, Value>, field: &str) -> Result<String, ManifestError> {
    let value = required_str(object, field)?;
    let chars = value.trim().chars().count();
    if chars == 0 || chars > MAX_NAME_CHARS {
        return Err(invalid(
            field,
            format!("must be 1–{MAX_NAME_CHARS} characters, got {chars}"),
        ));
    }
    Ok(value.trim().to_owned())
}

/// The payload for the manifest's `runtime`, removed from `object`.
fn payload(object: &mut Map<String, Value>) -> Result<Payload, ManifestError> {
    let runtime = required_str(object, "runtime")?.to_owned();
    let has_url = object.contains_key("payload_url");
    let payload = match runtime.as_str() {
        "luau" => {
            if object.contains_key("declarative") {
                return Err(invalid(
                    "declarative",
                    "only allowed with runtime \"declarative\"",
                ));
            }
            match object.remove("payload") {
                Some(Value::String(source)) => Payload::Luau(source),
                Some(other) => {
                    return Err(invalid(
                        "payload",
                        format!("expected Luau source as a string, got {}", describe(&other)),
                    ));
                }
                None if has_url => return Err(ManifestError::PayloadNotFetched("payload")),
                None => {
                    return Err(invalid(
                        "payload",
                        "missing; runtime \"luau\" needs the Luau source",
                    ));
                }
            }
        }
        "declarative" => {
            if object.contains_key("payload") {
                return Err(invalid("payload", "only allowed with runtime \"luau\""));
            }
            match object.remove("declarative") {
                Some(Value::Object(definition)) => Payload::Declarative(definition),
                Some(other) => {
                    return Err(invalid(
                        "declarative",
                        format!("expected an object, got {}", describe(&other)),
                    ));
                }
                None if has_url => return Err(ManifestError::PayloadNotFetched("declarative")),
                None => {
                    return Err(invalid(
                        "declarative",
                        "missing; runtime \"declarative\" needs the definition object",
                    ));
                }
            }
        }
        "wasm" => {
            return Err(invalid(
                "runtime",
                "\"wasm\" is not supported yet; use \"luau\" or \"declarative\"",
            ));
        }
        other => {
            return Err(invalid(
                "runtime",
                format!("{other:?} is not a runtime; use \"luau\" or \"declarative\""),
            ));
        }
    };
    let stored = payload.to_stored();
    let (field, empty) = match &payload {
        Payload::Luau(source) => ("payload", source.trim().is_empty()),
        Payload::Declarative(definition) => ("declarative", definition.is_empty()),
    };
    if empty {
        return Err(invalid(field, "must not be empty"));
    }
    if stored.len() > MAX_PAYLOAD_BYTES {
        return Err(ManifestError::TooLarge {
            part: "payload",
            size: stored.len(),
            limit: MAX_PAYLOAD_BYTES,
        });
    }
    Ok(payload)
}

fn hosts(object: &Map<String, Value>) -> Result<Vec<DeclaredHost>, ManifestError> {
    let permissions = match object.get("permissions") {
        Some(Value::Object(permissions)) => permissions,
        Some(other) => {
            return Err(invalid(
                "permissions",
                format!("expected an object, got {}", describe(other)),
            ));
        }
        None => {
            return Err(invalid(
                "permissions",
                "missing; declare the hosts the extension contacts in permissions.hosts",
            ));
        }
    };
    if let Some(unknown) = permissions.keys().find(|key| key.as_str() != "hosts") {
        return Err(invalid(
            format!("permissions.{unknown}"),
            "unknown permission; the only permission is hosts",
        ));
    }
    let hosts = match permissions.get("hosts") {
        Some(Value::Array(hosts)) => hosts,
        Some(other) => {
            return Err(invalid(
                "permissions.hosts",
                format!(
                    "expected an array of host patterns, got {}",
                    describe(other)
                ),
            ));
        }
        None => {
            return Err(invalid(
                "permissions.hosts",
                "missing; list the hosts the extension contacts, or []",
            ));
        }
    };
    hosts
        .iter()
        .enumerate()
        .map(|(index, host)| {
            let field = format!("permissions.hosts[{index}]");
            let pattern = host.as_str().ok_or_else(|| {
                invalid(
                    &field,
                    format!("expected a host pattern, got {}", describe(host)),
                )
            })?;
            DeclaredHost::parse(pattern).map_err(|error| invalid(&field, error.to_string()))
        })
        .collect()
}

fn sha256(object: &Map<String, Value>) -> Result<Option<String>, ManifestError> {
    let Some(hash) = optional_str(object, "sha256")? else {
        return Ok(None);
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("sha256", "expected 64 hex characters"));
    }
    Ok(Some(hash.to_ascii_lowercase()))
}

fn settings(object: &Map<String, Value>) -> Result<Vec<SettingField>, ManifestError> {
    let fields = match object.get("settings") {
        None => return Ok(Vec::new()),
        Some(Value::Array(fields)) => fields,
        Some(other) => {
            return Err(invalid(
                "settings",
                format!("expected an array of fields, got {}", describe(other)),
            ));
        }
    };
    let mut parsed: Vec<SettingField> = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        let path = format!("settings[{index}]");
        let Value::Object(field) = field else {
            return Err(invalid(
                path,
                format!("expected an object, got {}", describe(field)),
            ));
        };
        let setting = setting(&path, field)?;
        if parsed.iter().any(|other| other.key == setting.key) {
            return Err(invalid(
                format!("{path}.key"),
                format!("{:?} is used by another setting", setting.key),
            ));
        }
        parsed.push(setting);
    }
    Ok(parsed)
}

fn setting(path: &str, field: &Map<String, Value>) -> Result<SettingField, ManifestError> {
    let at = |name: &str| format!("{path}.{name}");
    let key = string_at(field, "key", &at("key"))?;
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(invalid(
            at("key"),
            format!("{key:?} must be one or more of a-z, 0-9 and _"),
        ));
    }
    let label = string_at(field, "label", &at("label"))?;
    if label.trim().is_empty() {
        return Err(invalid(at("label"), "must not be empty"));
    }
    let description = optional_string_at(field, "description", &at("description"))?;
    let required = optional_bool_at(field, "required", &at("required"))?.unwrap_or(false);
    let kind_name = string_at(field, "type", &at("type"))?;
    let (kind, extra): (SettingKind, &[&str]) = match kind_name.as_str() {
        "text" => (
            SettingKind::Text {
                default: optional_string_at(field, "default", &at("default"))?,
            },
            &["default"],
        ),
        "number" => (number(field, &at)?, &["min", "max", "integer", "default"]),
        "toggle" => (
            SettingKind::Toggle {
                default: optional_bool_at(field, "default", &at("default"))?,
            },
            &["default"],
        ),
        "select" => (select(field, &at)?, &["options", "default"]),
        "secret" => {
            if field.contains_key("default") {
                return Err(invalid(
                    at("default"),
                    "a secret setting can't have a default",
                ));
            }
            (SettingKind::Secret, &[])
        }
        other => {
            return Err(invalid(
                at("type"),
                format!(
                    "{other:?} is not a setting type; use text, number, toggle, select or secret"
                ),
            ));
        }
    };
    let common = ["key", "label", "type", "description", "required"];
    if let Some(unknown) = field
        .keys()
        .find(|key| !common.contains(&key.as_str()) && !extra.contains(&key.as_str()))
    {
        return Err(invalid(
            at(unknown),
            format!("not a field of a {kind_name} setting"),
        ));
    }
    Ok(SettingField {
        key,
        label,
        description,
        required,
        kind,
    })
}

fn number(
    field: &Map<String, Value>,
    at: &dyn Fn(&str) -> String,
) -> Result<SettingKind, ManifestError> {
    let min = optional_number_at(field, "min", &at("min"))?;
    let max = optional_number_at(field, "max", &at("max"))?;
    let integer = optional_bool_at(field, "integer", &at("integer"))?.unwrap_or(false);
    let default = optional_number_at(field, "default", &at("default"))?;
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        return Err(invalid(at("max"), format!("{max} is below min {min}")));
    }
    if let Some(default) = default {
        if min.is_some_and(|min| default < min) || max.is_some_and(|max| default > max) {
            return Err(invalid(
                at("default"),
                format!("{default} is outside min and max"),
            ));
        }
        if integer && default.fract() != 0.0 {
            return Err(invalid(
                at("default"),
                format!("{default} is not a whole number"),
            ));
        }
    }
    Ok(SettingKind::Number {
        min,
        max,
        integer,
        default,
    })
}

fn select(
    field: &Map<String, Value>,
    at: &dyn Fn(&str) -> String,
) -> Result<SettingKind, ManifestError> {
    let options = match field.get("options") {
        Some(Value::Array(options)) if !options.is_empty() => options,
        Some(Value::Array(_)) => {
            return Err(invalid(at("options"), "must list at least one option"));
        }
        Some(other) => {
            return Err(invalid(
                at("options"),
                format!("expected an array, got {}", describe(other)),
            ));
        }
        None => {
            return Err(invalid(
                at("options"),
                "missing; a select setting lists its options",
            ));
        }
    };
    let mut parsed: Vec<SelectOption> = Vec::with_capacity(options.len());
    for (index, option) in options.iter().enumerate() {
        let path = format!("{}[{index}]", at("options"));
        let Value::Object(option) = option else {
            return Err(invalid(
                path,
                format!("expected {{ value, label }}, got {}", describe(option)),
            ));
        };
        let value = string_at(option, "value", &format!("{path}.value"))?;
        let label = string_at(option, "label", &format!("{path}.label"))?;
        if let Some(unknown) = option
            .keys()
            .find(|key| !["value", "label"].contains(&key.as_str()))
        {
            return Err(invalid(
                format!("{path}.{unknown}"),
                "an option has only value and label",
            ));
        }
        if parsed.iter().any(|other| other.value == value) {
            return Err(invalid(
                format!("{path}.value"),
                format!("{value:?} is used by another option"),
            ));
        }
        parsed.push(SelectOption { value, label });
    }
    let default = optional_string_at(field, "default", &at("default"))?;
    if let Some(default) = &default
        && !parsed.iter().any(|option| &option.value == default)
    {
        return Err(invalid(
            at("default"),
            format!("{default:?} is not one of the option values"),
        ));
    }
    Ok(SettingKind::Select {
        options: parsed,
        default,
    })
}

fn required_str<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a str, ManifestError> {
    match object.get(field) {
        Some(Value::String(value)) => Ok(value),
        Some(other) => Err(invalid(
            field,
            format!("expected a string, got {}", describe(other)),
        )),
        None => Err(invalid(field, "missing")),
    }
}

fn optional_str(object: &Map<String, Value>, field: &str) -> Result<Option<String>, ManifestError> {
    optional_string_at(object, field, field)
}

fn optional_version(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<Version>, ManifestError> {
    optional_str(object, field)?
        .map(|text| {
            Version::parse(&text)
                .map_err(|error| invalid(field, format!("not a SemVer version: {error}")))
        })
        .transpose()
}

fn optional_https(object: &Map<String, Value>, field: &str) -> Result<Option<Url>, ManifestError> {
    optional_str(object, field)?
        .map(|text| {
            let url =
                Url::parse(&text).map_err(|error| invalid(field, format!("not a URL: {error}")))?;
            if url.scheme() != "https" {
                return Err(invalid(
                    field,
                    format!("must be an https URL, got {}:", url.scheme()),
                ));
            }
            Ok(url)
        })
        .transpose()
}

fn string_at(object: &Map<String, Value>, key: &str, path: &str) -> Result<String, ManifestError> {
    optional_string_at(object, key, path)?.ok_or_else(|| invalid(path, "missing"))
}

fn optional_string_at(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<String>, ManifestError> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Err(invalid(
            path,
            format!("expected a string, got {}", describe(other)),
        )),
    }
}

fn optional_bool_at(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<bool>, ManifestError> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(other) => Err(invalid(
            path,
            format!("expected true or false, got {}", describe(other)),
        )),
    }
}

fn optional_number_at(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<f64>, ManifestError> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Number(value)) => Ok(value.as_f64()),
        Some(other) => Err(invalid(
            path,
            format!("expected a number, got {}", describe(other)),
        )),
    }
}

/// A JSON value's type, with the value for short scalars.
fn describe(value: &Value) -> String {
    match value {
        Value::String(text) if text.chars().count() <= 40 => format!("the string {text:?}"),
        Value::Number(number) => format!("the number {number}"),
        Value::Bool(flag) => format!("{flag}"),
        other => json_type(other).to_owned(),
    }
}

const fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}
