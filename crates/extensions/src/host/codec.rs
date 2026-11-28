//! `autumn.json`, `url`, `base64`, `hex` and `crypto`: pure conversions.

use std::fmt::Write as _;

use base64::Engine as _;
use mlua::{
    LuaSerdeExt, LuaString, Table, Value,
    serde::{DeserializeOptions, SerializeOptions},
};
use sha1::Digest as _;

use super::{Builder, HostError};

/// `autumn.json.decode(s)` and `encode(v)`. JSON `null` decodes to `nil`.
pub(super) fn json(build: &Builder<'_>) -> mlua::Result<Table> {
    build.table([
        (
            "decode",
            build.function("autumn.json.decode", |lua, text: LuaString| {
                let json: serde_json::Value = serde_json::from_slice(&text.as_bytes())
                    .map_err(|error| HostError::parse(error.to_string()))?;
                let options = SerializeOptions::new()
                    .serialize_none_to_null(false)
                    .serialize_unit_to_null(false)
                    .set_array_metatable(false);
                Ok(lua.to_value_with(&json, options)?)
            })?,
        ),
        (
            "encode",
            build.function("autumn.json.encode", |lua, value: Value| {
                let json: serde_json::Value = lua
                    .from_value_with(value, DeserializeOptions::new())
                    .map_err(|error| HostError::invalid(error.to_string()))?;
                serde_json::to_string(&json).map_err(|error| HostError::invalid(error.to_string()))
            })?,
        ),
    ])
}

/// `autumn.url.encode/decode` (one component), `build` and `resolve`.
pub(super) fn url(build: &Builder<'_>) -> mlua::Result<Table> {
    build.table([
        (
            "encode",
            build.function("autumn.url.encode", |_, text: LuaString| {
                Ok(percent_encode(&text.as_bytes()))
            })?,
        ),
        (
            "decode",
            build.function("autumn.url.decode", |lua, text: LuaString| {
                Ok(lua.create_string(percent_decode(&text.as_bytes()))?)
            })?,
        ),
        (
            "build",
            build.function("autumn.url.build", |_, (base, query): (String, Table)| {
                build_url(&base, &query)
            })?,
        ),
        (
            "resolve",
            build.function("autumn.url.resolve", |_, (base, href): (String, String)| {
                let base = url::Url::parse(&base)
                    .map_err(|error| HostError::invalid(format!("base: {error}")))?;
                base.join(&href)
                    .map(String::from)
                    .map_err(|error| HostError::invalid(format!("href: {error}")))
            })?,
        ),
    ])
}

/// `autumn.base64.encode/decode`, standard alphabet with padding.
pub(super) fn base64(build: &Builder<'_>) -> mlua::Result<Table> {
    use base64::engine::general_purpose::STANDARD;
    build.table([
        (
            "encode",
            build.function("autumn.base64.encode", |_, bytes: LuaString| {
                Ok(STANDARD.encode(bytes.as_bytes()))
            })?,
        ),
        (
            "decode",
            build.function("autumn.base64.decode", |lua, text: LuaString| {
                let bytes = STANDARD
                    .decode(text.as_bytes())
                    .map_err(|error| HostError::parse(error.to_string()))?;
                Ok(lua.create_string(bytes)?)
            })?,
        ),
    ])
}

/// `autumn.hex.encode/decode`; encoding is lowercase, decoding either case.
pub(super) fn hex(build: &Builder<'_>) -> mlua::Result<Table> {
    build.table([
        (
            "encode",
            build.function("autumn.hex.encode", |_, bytes: LuaString| {
                Ok(to_hex(&bytes.as_bytes()))
            })?,
        ),
        (
            "decode",
            build.function("autumn.hex.decode", |lua, text: LuaString| {
                let bytes = from_hex(&text.as_bytes())
                    .ok_or_else(|| HostError::parse("expected an even number of hex digits"))?;
                Ok(lua.create_string(bytes)?)
            })?,
        ),
    ])
}

/// `autumn.crypto.sha1/sha256`, as lowercase hex.
pub(super) fn crypto(build: &Builder<'_>) -> mlua::Result<Table> {
    build.table([
        (
            "sha1",
            build.function("autumn.crypto.sha1", |_, bytes: LuaString| {
                Ok(to_hex(&sha1::Sha1::digest(bytes.as_bytes())))
            })?,
        ),
        (
            "sha256",
            build.function("autumn.crypto.sha256", |_, bytes: LuaString| {
                use sha2::Digest as _;
                Ok(to_hex(&sha2::Sha256::digest(bytes.as_bytes())))
            })?,
        ),
    ])
}

/// Appends `query` to `base` with its keys sorted, so the URL is
/// deterministic. Values must be strings, numbers or booleans.
fn build_url(base: &str, query: &Table) -> Result<String, HostError> {
    let mut url =
        url::Url::parse(base).map_err(|error| HostError::invalid(format!("base: {error}")))?;
    let mut pairs = Vec::new();
    for pair in query.pairs::<String, Value>() {
        let (key, value) = pair.map_err(|error| HostError::invalid(format!("query: {error}")))?;
        let value = match value {
            Value::String(value) => value.to_string_lossy(),
            Value::Integer(_) | Value::Number(_) | Value::Boolean(_) => value.to_string()?,
            other => {
                return Err(HostError::invalid(format!(
                    "query.{key}: expected string, number or boolean, got {}",
                    other.type_name()
                )));
            }
        };
        pairs.push((key, value));
    }
    pairs.sort();
    if !pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(pairs);
    }
    Ok(url.into())
}

/// Percent-encodes every byte outside RFC 3986's unreserved set.
fn percent_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            // Writing to a String cannot fail.
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Decodes `%XX` sequences; a malformed one is kept as written.
fn percent_decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%'
            && let [high, low, after @ ..] = tail
            && let (Some(high), Some(low)) = (hex_digit(*high), hex_digit(*low))
        {
            out.push(high << 4 | low);
            rest = after;
            continue;
        }
        out.push(byte);
        rest = tail;
    }
    out
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn from_hex(text: &[u8]) -> Option<Vec<u8>> {
    let (pairs, []) = text.as_chunks::<2>() else {
        return None;
    };
    pairs
        .iter()
        .map(|[high, low]| Some(hex_digit(*high)? << 4 | hex_digit(*low)?))
        .collect()
}

fn hex_digit(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use mlua::Value;

    use crate::host::test_support::{eval, raised};

    #[test]
    fn json_round_trips_and_null_is_nil() {
        let (name, missing, count, flag): (String, Value, i64, bool) = eval(
            r#"
            local v = autumn.json.decode('{"name":"x","gone":null,"n":3,"ok":true}')
            return v.name, v.gone, v.n, v.ok
            "#,
        )
        .unwrap();
        assert_eq!((name.as_str(), count, flag), ("x", 3, true));
        assert_eq!(missing, Value::Nil, "null decodes to nil, not a sentinel");

        let encoded: String =
            eval(r#"return autumn.json.encode({ b = 1, a = { "x", "y" } })"#).unwrap();
        assert_eq!(encoded, r#"{"a":["x","y"],"b":1}"#);

        let error = raised("autumn.json.decode('{')");
        assert!(error.starts_with("autumn.json.decode: parse: "), "{error}");
        let error = raised("autumn.json.encode({ f = print })");
        assert!(
            error.starts_with("autumn.json.encode: invalid_argument: "),
            "{error}"
        );
    }

    #[test]
    fn url_encodes_one_component_and_decodes_it_back() {
        let (encoded, decoded): (String, String) =
            eval("local e = autumn.url.encode('a b/ü&~') return e, autumn.url.decode(e)").unwrap();
        assert_eq!(encoded, "a%20b%2F%C3%BC%26~");
        assert_eq!(decoded, "a b/ü&~");
        let kept: String = eval("return autumn.url.decode('100%zz%4')").unwrap();
        assert_eq!(kept, "100%zz%4", "malformed escapes are kept");
    }

    #[test]
    fn url_build_sorts_keys_and_resolve_follows_links() {
        let built: String = eval(
            "return autumn.url.build('https://nyaa.si/?page=rss', { q = 'one piece', c = '1_2', f = 0 })",
        )
        .unwrap();
        assert_eq!(built, "https://nyaa.si/?page=rss&c=1_2&f=0&q=one+piece");

        let resolved: String =
            eval("return autumn.url.resolve('https://nyaa.si/view/1', '/download/1.torrent')")
                .unwrap();
        assert_eq!(resolved, "https://nyaa.si/download/1.torrent");

        let error = raised("autumn.url.build('not a url', {})");
        assert!(
            error.starts_with("autumn.url.build: invalid_argument: base:"),
            "{error}"
        );
        let error = raised("autumn.url.build('https://x.test', { q = {} })");
        assert!(error.contains("query.q: expected string"), "{error}");
    }

    #[test]
    fn base64_and_hex_round_trip_bytes() {
        let (b64, hex, round_trips): (String, String, bool) = eval(
            r#"
            local bytes = "hi\0\171!"
            local b = autumn.base64.encode(bytes)
            local h = autumn.hex.encode(bytes)
            local back = autumn.base64.decode(b) == bytes
                and autumn.hex.decode(h) == bytes
                and autumn.hex.decode(string.upper(h)) == bytes
            return b, h, back
            "#,
        )
        .unwrap();
        assert_eq!(b64, "aGkAqyE=");
        assert_eq!(hex, "686900ab21");
        assert!(
            round_trips,
            "both decode back to the same bytes, hex in either case"
        );

        let error = raised("autumn.hex.decode('abc')");
        assert!(error.starts_with("autumn.hex.decode: parse: "), "{error}");
        let error = raised("autumn.base64.decode('***')");
        assert!(
            error.starts_with("autumn.base64.decode: parse: "),
            "{error}"
        );
    }

    #[test]
    fn crypto_hashes_are_lowercase_hex() {
        let (sha1, sha256): (String, String) =
            eval("return autumn.crypto.sha1('abc'), autumn.crypto.sha256('abc')").unwrap();
        assert_eq!(sha1, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
