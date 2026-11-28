//! Every manifest rule and cap (#299, ADR 0012).

#![expect(
    clippy::panic,
    clippy::unreachable,
    reason = "test helpers panic when the case under test fails"
)]

use extensions::{
    ExtensionKind, MAX_DESCRIPTOR_BYTES, MAX_PAYLOAD_BYTES, Manifest, ManifestError, Payload,
    SelectOption, SettingKind,
};
use serde_json::{Value, json};

/// One field set (`Some`) or removed (`None`).
type Change = (&'static str, Option<Value>);

/// A valid Luau manifest to change one field at a time.
fn valid() -> Value {
    json!({
        "id": "my-ext",
        "name": "My Extension",
        "author": "someone",
        "version": "1.2.3",
        "api_version": 1,
        "type": "torrent-provider",
        "runtime": "luau",
        "payload": "return { search = function() return {} end }",
        "permissions": { "hosts": ["nyaa.si", "*.example.org"] },
    })
}

fn with(changes: &[(&str, Option<Value>)]) -> String {
    let mut manifest = valid();
    let Value::Object(object) = &mut manifest else {
        unreachable!("valid() is an object");
    };
    for (key, value) in changes {
        match value {
            Some(value) => object.insert((*key).to_owned(), value.clone()),
            None => object.remove(*key),
        };
    }
    manifest.to_string()
}

/// The `field: problem` text of an `Invalid` error.
fn invalid(changes: &[(&str, Option<Value>)]) -> String {
    match Manifest::parse(&with(changes)) {
        Err(error @ ManifestError::Invalid { .. }) => error.to_string(),
        other => panic!("expected Invalid for {changes:?}, got {other:?}"),
    }
}

#[test]
fn a_valid_manifest_parses_with_every_optional_field() {
    let manifest = Manifest::parse(&with(&[
        ("description", Some(json!("Searches things"))),
        ("icon", Some(json!("https://example.org/icon.png"))),
        ("website", Some(json!("https://example.org"))),
        ("license", Some(json!("MIT"))),
        ("lang", Some(json!("en"))),
        ("notes", Some(json!("Needs an account"))),
        ("min_app_version", Some(json!("0.0.1"))),
        ("sha256", Some(json!("AB".repeat(32)))),
        (
            "payload_url",
            Some(json!("https://example.org/my-ext.luau")),
        ),
        ("future_field", Some(json!(true))),
    ]))
    .unwrap();
    assert_eq!(manifest.id.as_str(), "my-ext");
    assert_eq!(manifest.name, "My Extension");
    assert_eq!(manifest.version.to_string(), "1.2.3");
    assert_eq!(manifest.api_version, 1);
    assert_eq!(manifest.kind, ExtensionKind::TorrentProvider);
    assert!(matches!(&manifest.payload, Payload::Luau(source) if source.starts_with("return")));
    let hosts: Vec<String> = manifest.hosts.iter().map(ToString::to_string).collect();
    assert_eq!(hosts, ["nyaa.si", "*.example.org"]);
    assert_eq!(
        manifest.sha256.as_deref(),
        Some("ab".repeat(32).as_str()),
        "lowercased"
    );
    assert_eq!(manifest.lang.as_deref(), Some("en"));
}

#[test]
fn ids_follow_the_slug_rule() {
    for id in ["abc", "my-ext-2", &"a".repeat(64), "0ab"] {
        assert!(
            Manifest::parse(&with(&[("id", Some(json!(id)))])).is_ok(),
            "{id}"
        );
    }
    for id in [
        "ab",
        "-abc",
        "abc-",
        "My-Ext",
        "my_ext",
        "my ext",
        &"a".repeat(65),
    ] {
        let error = invalid(&[("id", Some(json!(id)))]);
        assert!(error.starts_with("id: "), "{id}: {error}");
    }
}

#[test]
fn name_and_author_are_1_to_64_characters() {
    for field in ["name", "author"] {
        assert!(Manifest::parse(&with(&[(field, Some(json!("é".repeat(64))))])).is_ok());
        for bad in [json!(""), json!("   "), json!("x".repeat(65)), json!(5)] {
            let error = invalid(&[(field, Some(bad.clone()))]);
            assert!(error.starts_with(&format!("{field}: ")), "{bad}: {error}");
        }
        assert_eq!(invalid(&[(field, None)]), format!("{field}: missing"));
    }
}

#[test]
fn versions_are_strict_semver_compared_by_precedence() {
    for bad in ["1.0", "v1.0.0", "1.0.0.0", "01.0.0"] {
        let error = invalid(&[("version", Some(json!(bad)))]);
        assert!(
            error.starts_with("version: not a SemVer version"),
            "{bad}: {error}"
        );
    }
    let manifest = Manifest::parse(&with(&[("version", Some(json!("1.2.3+build.5")))])).unwrap();
    let same = semver::Version::parse("1.2.3+other").unwrap();
    let older = semver::Version::parse("1.2.3-beta").unwrap();
    assert_eq!(
        manifest.cmp_version(&same),
        std::cmp::Ordering::Equal,
        "build metadata ignored"
    );
    assert_eq!(manifest.cmp_version(&older), std::cmp::Ordering::Greater);
}

#[test]
fn unsupported_api_versions_are_incompatible_before_other_checks() {
    let error = Manifest::parse(&with(&[
        ("api_version", Some(json!(2))),
        ("id", Some(json!("BAD"))),
    ]))
    .unwrap_err();
    assert_eq!(error, ManifestError::UnsupportedApiVersion { found: 2 });
    assert_eq!(
        error.to_string(),
        "api_version 2 is not supported; this version of Autumn runs api_version 1"
    );
    for bad in [json!("1"), json!(1.5), json!(-1)] {
        let error = invalid(&[("api_version", Some(bad.clone()))]);
        assert!(
            error.starts_with("api_version: expected a whole number"),
            "{bad}: {error}"
        );
    }
    assert!(invalid(&[("api_version", None)]).starts_with("api_version: missing"));
}

#[test]
fn min_app_version_above_this_app_is_incompatible() {
    let error = Manifest::parse(&with(&[("min_app_version", Some(json!("999.0.0")))])).unwrap_err();
    assert!(
        matches!(error, ManifestError::AppTooOld { .. }),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .starts_with("the extension needs Autumn 999.0.0 or newer")
    );
    let error = invalid(&[("min_app_version", Some(json!("soon")))]);
    assert!(
        error.starts_with("min_app_version: not a SemVer version"),
        "{error}"
    );
}

#[test]
fn only_torrent_providers_on_luau_or_declarative() {
    assert_eq!(
        invalid(&[("type", Some(json!("online-stream")))]),
        r#"type: "online-stream" is not supported; expected "torrent-provider""#
    );
    assert_eq!(
        invalid(&[("runtime", Some(json!("wasm")))]),
        r#"runtime: "wasm" is not supported yet; use "luau" or "declarative""#
    );
    assert!(
        invalid(&[("runtime", Some(json!("js")))]).starts_with("runtime: \"js\" is not a runtime")
    );
}

#[test]
fn payloads_match_their_runtime() {
    let declarative = Manifest::parse(&with(&[
        ("runtime", Some(json!("declarative"))),
        ("payload", None),
        (
            "declarative",
            Some(json!({ "request": { "url": "https://nyaa.si/" } })),
        ),
    ]))
    .unwrap();
    assert!(matches!(declarative.payload, Payload::Declarative(_)));

    let cases: [(&[Change], &str); 6] = [
        (
            &[("payload", None)],
            "payload: missing; runtime \"luau\" needs the Luau source",
        ),
        (
            &[("payload", Some(json!("  \n")))],
            "payload: must not be empty",
        ),
        (
            &[("payload", Some(json!({})))],
            "payload: expected Luau source as a string, got an object",
        ),
        (
            &[("declarative", Some(json!({ "x": 1 })))],
            "declarative: only allowed with runtime \"declarative\"",
        ),
        (
            &[
                ("runtime", Some(json!("declarative"))),
                ("payload", None),
                ("declarative", Some(json!({}))),
            ],
            "declarative: must not be empty",
        ),
        (
            &[("runtime", Some(json!("declarative"))), ("payload", None)],
            "declarative: missing; runtime \"declarative\" needs the definition object",
        ),
    ];
    for (changes, expected) in cases {
        assert_eq!(invalid(changes), expected, "{changes:?}");
    }
}

#[test]
fn a_payload_url_alone_must_be_fetched_first() {
    let error = Manifest::parse(&with(&[
        ("payload", None),
        ("payload_url", Some(json!("https://example.org/x.luau"))),
    ]))
    .unwrap_err();
    assert_eq!(error, ManifestError::PayloadNotFetched("payload"));
    for (field, url) in [
        ("payload_url", "http://example.org/x.luau"),
        ("icon", "ftp://example.org/i.png"),
        ("website", "file:///etc/passwd"),
    ] {
        let error = invalid(&[(field, Some(json!(url)))]);
        assert!(
            error.starts_with(&format!("{field}: must be an https URL")),
            "{error}"
        );
    }
    assert!(invalid(&[("website", Some(json!("not a url")))]).starts_with("website: not a URL"));
}

#[test]
fn permissions_declare_valid_hosts_and_nothing_else() {
    assert!(Manifest::parse(&with(&[("permissions", Some(json!({ "hosts": [] })))])).is_ok());
    let cases = [
        (
            None,
            "permissions: missing; declare the hosts the extension contacts in permissions.hosts",
        ),
        (
            Some(json!([])),
            "permissions: expected an object, got an array",
        ),
        (
            Some(json!({})),
            "permissions.hosts: missing; list the hosts the extension contacts, or []",
        ),
        (
            Some(json!({ "hosts": "nyaa.si" })),
            "permissions.hosts: expected an array of host patterns, got the string \"nyaa.si\"",
        ),
        (
            Some(json!({ "hosts": [], "filesystem": true })),
            "permissions.filesystem: unknown permission; the only permission is hosts",
        ),
        (
            Some(json!({ "hosts": [5] })),
            "permissions.hosts[0]: expected a host pattern, got the number 5",
        ),
    ];
    for (permissions, expected) in cases {
        assert_eq!(
            invalid(&[("permissions", permissions.clone())]),
            expected,
            "{permissions:?}"
        );
    }
    for pattern in ["*", "*.com", "", "http://nyaa.si"] {
        let error = invalid(&[(
            "permissions",
            Some(json!({ "hosts": ["nyaa.si", pattern] })),
        )]);
        assert!(
            error.starts_with("permissions.hosts[1]: "),
            "{pattern}: {error}"
        );
    }
}

#[test]
fn sha256_is_64_hex_characters() {
    for bad in ["abc", &"g".repeat(64), &"a".repeat(63)] {
        assert_eq!(
            invalid(&[("sha256", Some(json!(bad)))]),
            "sha256: expected 64 hex characters"
        );
    }
}

#[test]
fn the_descriptor_and_payload_are_capped() {
    let big_notes = "x".repeat(MAX_DESCRIPTOR_BYTES);
    let error = Manifest::parse(&with(&[("notes", Some(json!(big_notes)))])).unwrap_err();
    assert!(
        matches!(
            error,
            ManifestError::TooLarge {
                part: "manifest",
                limit: MAX_DESCRIPTOR_BYTES,
                ..
            }
        ),
        "{error:?}"
    );

    // The payload doesn't count toward the descriptor, and has its own cap.
    let at_cap = format!("return {{}}{}", " ".repeat(MAX_PAYLOAD_BYTES - 9));
    assert!(Manifest::parse(&with(&[("payload", Some(json!(at_cap)))])).is_ok());
    let over = format!("{at_cap} ");
    let error = Manifest::parse(&with(&[("payload", Some(json!(over)))])).unwrap_err();
    assert_eq!(
        error,
        ManifestError::TooLarge {
            part: "payload",
            size: MAX_PAYLOAD_BYTES + 1,
            limit: MAX_PAYLOAD_BYTES
        }
    );
}

#[test]
fn not_json_or_not_an_object_is_refused() {
    assert!(matches!(Manifest::parse("{"), Err(ManifestError::Json(_))));
    let error = Manifest::parse("[]").unwrap_err();
    assert_eq!(
        error.to_string(),
        "the manifest is not a JSON object: expected an object, got an array"
    );
}

fn settings(fields: Value) -> Result<Manifest, ManifestError> {
    Manifest::parse(&with(&[("settings", Some(fields))]))
}

#[test]
fn every_setting_type_parses() {
    let manifest = settings(json!([
        { "key": "user", "label": "User", "type": "text", "default": "anon", "description": "Your name" },
        { "key": "limit", "label": "Limit", "type": "number", "min": 1, "max": 50, "integer": true, "default": 10 },
        { "key": "dub", "label": "Dub", "type": "toggle", "default": false, "required": true },
        { "key": "sort", "label": "Sort", "type": "select", "default": "seeders",
          "options": [{ "value": "seeders", "label": "Seeders" }, { "value": "date", "label": "Date" }] },
        { "key": "api_key", "label": "API key", "type": "secret" },
    ]))
    .unwrap();
    let kinds: Vec<_> = manifest.settings.iter().map(|field| &field.kind).collect();
    assert_eq!(
        kinds,
        [
            &SettingKind::Text {
                default: Some("anon".to_owned())
            },
            &SettingKind::Number {
                min: Some(1.0),
                max: Some(50.0),
                integer: true,
                default: Some(10.0)
            },
            &SettingKind::Toggle {
                default: Some(false)
            },
            &SettingKind::Select {
                options: vec![
                    SelectOption {
                        value: "seeders".to_owned(),
                        label: "Seeders".to_owned()
                    },
                    SelectOption {
                        value: "date".to_owned(),
                        label: "Date".to_owned()
                    },
                ],
                default: Some("seeders".to_owned()),
            },
            &SettingKind::Secret,
        ]
    );
    let required: Vec<bool> = manifest
        .settings
        .iter()
        .map(|field| field.required)
        .collect();
    assert_eq!(
        required,
        [false, false, true, false, false],
        "required defaults to false"
    );
}

#[test]
fn setting_rules_name_the_broken_field() {
    let text = |extra: Value| {
        let mut field = json!({ "key": "k", "label": "K", "type": "text" });
        if let (Value::Object(field), Value::Object(extra)) = (&mut field, extra) {
            field.extend(extra);
        }
        field
    };
    let cases = [
        (
            json!({}),
            "settings: expected an array of fields, got an object",
        ),
        (
            json!([5]),
            "settings[0]: expected an object, got the number 5",
        ),
        (
            json!([text(json!({ "key": "Bad-Key" }))]),
            "settings[0].key: \"Bad-Key\" must be one or more of a-z, 0-9 and _",
        ),
        (
            json!([text(json!({})), text(json!({}))]),
            "settings[1].key: \"k\" is used by another setting",
        ),
        (
            json!([text(json!({ "label": " " }))]),
            "settings[0].label: must not be empty",
        ),
        (
            json!([text(json!({ "type": "color" }))]),
            "settings[0].type: \"color\" is not a setting type; use text, number, toggle, select or secret",
        ),
        (
            json!([text(json!({ "required": "yes" }))]),
            "settings[0].required: expected true or false, got the string \"yes\"",
        ),
        (
            json!([text(json!({ "min": 1 }))]),
            "settings[0].min: not a field of a text setting",
        ),
        (
            json!([text(json!({ "type": "secret", "default": "x" }))]),
            "settings[0].default: a secret setting can't have a default",
        ),
        (
            json!([text(json!({ "type": "number", "min": 5, "max": 1 }))]),
            "settings[0].max: 1 is below min 5",
        ),
        (
            json!([text(json!({ "type": "number", "max": 3, "default": 4 }))]),
            "settings[0].default: 4 is outside min and max",
        ),
        (
            json!([text(
                json!({ "type": "number", "integer": true, "default": 1.5 })
            )]),
            "settings[0].default: 1.5 is not a whole number",
        ),
        (
            json!([text(json!({ "type": "toggle", "default": "true" }))]),
            "settings[0].default: expected true or false, got the string \"true\"",
        ),
        (
            json!([text(json!({ "type": "select" }))]),
            "settings[0].options: missing; a select setting lists its options",
        ),
        (
            json!([text(json!({ "type": "select", "options": [] }))]),
            "settings[0].options: must list at least one option",
        ),
        (
            json!([text(
                json!({ "type": "select", "options": [{ "value": "a" }] })
            )]),
            "settings[0].options[0].label: missing",
        ),
        (
            json!([text(
                json!({ "type": "select", "options": [{ "value": "a", "label": "A" }, { "value": "a", "label": "B" }] })
            )]),
            "settings[0].options[1].value: \"a\" is used by another option",
        ),
        (
            json!([text(
                json!({ "type": "select", "options": [{ "value": "a", "label": "A" }], "default": "b" })
            )]),
            "settings[0].default: \"b\" is not one of the option values",
        ),
    ];
    for (fields, expected) in cases {
        let error = settings(fields.clone()).unwrap_err();
        assert_eq!(error.to_string(), expected, "{fields}");
    }
}

#[test]
fn a_stored_manifest_round_trips() {
    for json in [
        with(&[(
            "settings",
            Some(json!([
                { "key": "sort", "label": "Sort", "type": "select", "options": [{ "value": "a", "label": "A" }] },
                { "key": "api_key", "label": "Key", "type": "secret", "required": true },
            ])),
        )]),
        with(&[
            ("runtime", Some(json!("declarative"))),
            ("payload", None),
            (
                "declarative",
                Some(json!({ "request": { "url": "https://nyaa.si/" } })),
            ),
        ]),
    ] {
        let manifest = Manifest::parse(&json).unwrap();
        let descriptor = manifest.descriptor();
        assert!(
            !descriptor.contains("\"payload\"") && !descriptor.contains("\"declarative\":"),
            "{descriptor}"
        );
        let stored = Manifest::from_stored(&descriptor, &manifest.payload.to_stored()).unwrap();
        assert_eq!(stored, manifest);
    }
}
