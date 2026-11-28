//! `autumn.time.now()` and `autumn.time.parse(s)`, in Unix seconds (UTC).

use mlua::Table;
use time::{
    OffsetDateTime, PrimitiveDateTime,
    format_description::{
        BorrowedFormatItem,
        well_known::{Rfc2822, Rfc3339},
    },
    macros::format_description,
};

use super::Builder;

/// `YYYY-MM-DD HH:MM:SS` and `YYYY-MM-DD HH:MM`, read as UTC.
const PLAIN_FORMATS: [&[BorrowedFormatItem<'_>]; 2] = [
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"),
    format_description!("[year]-[month]-[day] [hour]:[minute]"),
];

pub(super) fn module(build: &Builder<'_>) -> mlua::Result<Table> {
    build.table([
        (
            "now",
            build.function("autumn.time.now", |_, ()| {
                Ok(OffsetDateTime::now_utc().unix_timestamp())
            })?,
        ),
        (
            "parse",
            build.function("autumn.time.parse", |_, text: String| Ok(parse(&text)))?,
        ),
    ])
}

/// RFC 3339, RFC 2822, or a plain UTC date and time; `None` otherwise.
fn parse(text: &str) -> Option<i64> {
    let text = text.trim();
    OffsetDateTime::parse(text, &Rfc3339)
        .or_else(|_| OffsetDateTime::parse(text, &Rfc2822))
        .ok()
        .or_else(|| {
            PLAIN_FORMATS.iter().find_map(|format| {
                PrimitiveDateTime::parse(text, format)
                    .ok()
                    .map(PrimitiveDateTime::assume_utc)
            })
        })
        .map(OffsetDateTime::unix_timestamp)
}

#[cfg(test)]
mod tests {
    use mlua::Value;

    use crate::host::test_support::eval;

    #[test]
    fn parse_reads_each_format_as_unix_seconds() {
        let cases = [
            ("2024-01-02T03:04:05Z", 1_704_164_645),
            ("2024-01-02T05:04:05+02:00", 1_704_164_645),
            ("Tue, 02 Jan 2024 03:04:05 +0000", 1_704_164_645),
            ("2024-01-02 03:04:05", 1_704_164_645),
            ("2024-01-02 03:04", 1_704_164_640),
        ];
        for (text, expected) in cases {
            let seconds: i64 = eval(&format!("return autumn.time.parse({text:?})")).unwrap();
            assert_eq!(seconds, expected, "{text}");
        }
    }

    #[test]
    fn parse_gives_nil_for_anything_else() {
        for text in ["", "yesterday", "2024-01-02", "2024-13-01 00:00"] {
            let value: Value = eval(&format!("return autumn.time.parse({text:?})")).unwrap();
            assert_eq!(value, Value::Nil, "{text}");
        }
    }

    #[test]
    fn now_is_current_unix_seconds() {
        let now: i64 = eval("return autumn.time.now()").unwrap();
        let expected = time::OffsetDateTime::now_utc().unix_timestamp();
        assert!((expected - now).abs() <= 2, "{now} vs {expected}");
    }
}
