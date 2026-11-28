//! `autumn.html.select` and `autumn.html.extract`: CSS selection over one
//! parse, returning plain tables (no DOM userdata, which wouldn't be `Send`).

use mlua::{Lua, Table, Value};
use scraper::{ElementRef, Html, Selector};

use super::{Builder, HostError};

pub(super) fn module(build: &Builder<'_>) -> mlua::Result<Table> {
    build.table([
        (
            "select",
            build.function(
                "autumn.html.select",
                |lua, (html, css): (String, String)| select(lua, &html, &css),
            )?,
        ),
        (
            "extract",
            build.function(
                "autumn.html.extract",
                |lua, (html, row_css, fields): (String, String, Table)| {
                    extract(lua, &html, &row_css, &fields)
                },
            )?,
        ),
    ])
}

/// Every match of `css` as `{ text, html, attrs }`.
fn select(lua: &Lua, html: &str, css: &str) -> Result<Table, HostError> {
    let selector = selector(css, "css")?;
    let document = Html::parse_document(html);
    let matches = lua.create_table()?;
    for element in document.select(&selector) {
        let row = lua.create_table()?;
        row.raw_set("text", text(element))?;
        row.raw_set("html", element.html())?;
        let attrs = lua.create_table()?;
        for (name, value) in element.value().attrs() {
            attrs.raw_set(name, value)?;
        }
        row.raw_set("attrs", attrs)?;
        matches.raw_push(row)?;
    }
    Ok(matches)
}

/// One field of `extract`: where to look inside a row and what to take.
struct Field {
    name: String,
    selector: Selector,
    /// The attribute to read, or the element's text when `None`.
    attr: Option<String>,
    take: Take,
}

/// Whether a field keeps the first match or all of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Take {
    First,
    All,
}

/// One table per match of `row_css`, each field filled from inside that row.
fn extract(lua: &Lua, html: &str, row_css: &str, fields: &Table) -> Result<Table, HostError> {
    let rows = selector(row_css, "row_css")?;
    let fields = parse_fields(fields)?;
    let document = Html::parse_document(html);
    let out = lua.create_table()?;
    for row in document.select(&rows) {
        let record = lua.create_table()?;
        for field in &fields {
            let mut values = row
                .select(&field.selector)
                .map(|element| match &field.attr {
                    Some(attr) => element.value().attr(attr).map(str::to_owned),
                    None => Some(text(element)),
                });
            match field.take {
                Take::First => record.raw_set(field.name.as_str(), values.next().flatten())?,
                Take::All => {
                    let all = lua.create_sequence_from(values.flatten())?;
                    record.raw_set(field.name.as_str(), all)?;
                }
            }
        }
        out.raw_push(record)?;
    }
    Ok(out)
}

/// Reads `fields`: each is `"css"` or `{ css, attr = "href", all = false }`.
fn parse_fields(fields: &Table) -> Result<Vec<Field>, HostError> {
    let mut parsed = Vec::new();
    for pair in fields.pairs::<String, Value>() {
        let (name, spec) = pair.map_err(|error| HostError::invalid(format!("fields: {error}")))?;
        let path = format!("fields.{name}");
        let field = match spec {
            Value::String(css) => Field {
                selector: selector(&css.to_string_lossy(), &path)?,
                name,
                attr: None,
                take: Take::First,
            },
            Value::Table(spec) => {
                let css: String = spec
                    .raw_get("css")
                    .map_err(|_| HostError::invalid(format!("{path}.css: expected string")))?;
                let attr: Option<String> = spec
                    .raw_get("attr")
                    .map_err(|_| HostError::invalid(format!("{path}.attr: expected string")))?;
                let all = match spec.raw_get::<Value>("all")? {
                    Value::Nil | Value::Boolean(false) => Take::First,
                    Value::Boolean(true) => Take::All,
                    other => {
                        return Err(HostError::invalid(format!(
                            "{path}.all: expected boolean, got {}",
                            other.type_name()
                        )));
                    }
                };
                Field {
                    selector: selector(&css, &format!("{path}.css"))?,
                    name,
                    attr,
                    take: all,
                }
            }
            other => {
                return Err(HostError::invalid(format!(
                    "{path}: expected string or table, got {}",
                    other.type_name()
                )));
            }
        };
        parsed.push(field);
    }
    Ok(parsed)
}

fn selector(css: &str, path: &str) -> Result<Selector, HostError> {
    Selector::parse(css).map_err(|error| HostError::invalid(format!("{path}: {error}")))
}

/// The element's text content, trimmed.
fn text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_owned()
}

#[cfg(test)]
mod tests {
    use mlua::Value;

    use crate::host::test_support::{eval, raised};

    #[test]
    fn select_returns_text_html_and_attrs() {
        let (count, text, html, href): (i64, String, String, String) = eval(
            r#"
            local found = autumn.html.select(
                '<p><a class="x" href="/a"> One </a><a href="/b">Two</a></p>', "a")
            local first = found[1]
            return #found, first.text, first.html, first.attrs.href
            "#,
        )
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(text, "One");
        assert_eq!(html, r#"<a class="x" href="/a"> One </a>"#);
        assert_eq!(href, "/a");
    }

    #[test]
    fn extract_reads_cells_inside_table_rows() {
        let (rows, name, link, missing, tags): (i64, String, String, Value, String) = eval(
            r#"
            local html = [[
              <table><tbody>
                <tr><td class="n">Show 01</td><td><a href="/1">dl</a></td>
                    <td><i>a</i><i>b</i></td></tr>
                <tr><td class="n">Show 02</td><td></td><td></td></tr>
              </tbody></table>
            ]]
            local rows = autumn.html.extract(html, "tbody tr", {
                name = "td.n",
                link = { css = "a", attr = "href" },
                tags = { css = "i", all = true },
            })
            return #rows, rows[1].name, rows[1].link, rows[2].link,
                table.concat(rows[1].tags, ",") .. "|" .. #rows[2].tags
            "#,
        )
        .unwrap();
        assert_eq!(rows, 2);
        assert_eq!((name.as_str(), link.as_str()), ("Show 01", "/1"));
        assert_eq!(missing, Value::Nil, "a missing match is nil");
        assert_eq!(tags, "a,b|0");
    }

    #[test]
    fn bad_selectors_and_field_specs_are_invalid_arguments() {
        let error = raised("autumn.html.select('<p>', '[[')");
        assert!(
            error.starts_with("autumn.html.select: invalid_argument: css:"),
            "{error}"
        );
        let error = raised("autumn.html.extract('<p>', 'p', { x = 5 })");
        assert!(
            error.contains("fields.x: expected string or table, got integer"),
            "{error}"
        );
        let error = raised("autumn.html.extract('<p>', 'p', { x = { css = 'a', all = 'yes' } })");
        assert!(error.contains("fields.x.all: expected boolean"), "{error}");
    }
}
