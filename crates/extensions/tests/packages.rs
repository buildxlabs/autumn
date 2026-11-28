//! Installed packages in `autumn.db`, loaded back onto the host (#299).

#![expect(clippy::unwrap_used, reason = "test setup panics when it fails")]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use db::Db;
use domain::ExtensionId;
use extensions::{ExtensionHost, PackageError, Packages, ProviderRequest};
use serde_json::json;

const ORIGIN: &str = "https://example.org/index.json";

fn manifest(id: &str, runtime: &str) -> String {
    let mut manifest = json!({
        "id": id,
        "name": format!("{id} name"),
        "author": "someone",
        "version": "1.0.0",
        "api_version": 1,
        "type": "torrent-provider",
        "runtime": runtime,
        "permissions": { "hosts": ["nyaa.si"] },
    });
    let (key, payload) = match runtime {
        "luau" => (
            "payload",
            json!(
                r#"return { search = function(r) return { { name = "hit for " .. r.keyword } } end }"#
            ),
        ),
        _ => (
            "declarative",
            json!({ "request": { "url": "https://nyaa.si/?q={keyword}" } }),
        ),
    };
    if let Some(object) = manifest.as_object_mut() {
        object.insert(key.to_owned(), payload);
    }
    manifest.to_string()
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

async fn db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
    (dir, db)
}

#[tokio::test]
async fn an_installed_row_runs_as_a_luau_provider() {
    let (_dir, db) = db().await;
    let packages = Packages::new(&db);
    let installed = packages
        .install(&manifest("my-ext", "luau"), ORIGIN, at(1))
        .await
        .unwrap();
    assert_eq!(installed.id.as_str(), "my-ext");

    let sources = packages.runnable().await.unwrap();
    assert_eq!(sources.len(), 1);
    let host = ExtensionHost::start(sources);
    let info: Vec<_> = host
        .providers()
        .map(|info| (info.id.as_str(), info.name.as_str(), info.version.as_str()))
        .collect();
    assert_eq!(info, [("my-ext", "my-ext name", "1.0.0")]);
    let rows = host
        .search(
            &ExtensionId::new("my-ext"),
            ProviderRequest::keyword("frieren"),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.first().and_then(|row| row.name.as_deref()),
        Some("hit for frieren")
    );
}

#[tokio::test]
async fn install_list_disable_and_uninstall() {
    let (_dir, db) = db().await;
    let packages = Packages::new(&db);
    packages
        .install(&manifest("luau-ext", "luau"), ORIGIN, at(1))
        .await
        .unwrap();
    packages
        .install(&manifest("decl-ext", "declarative"), ORIGIN, at(2))
        .await
        .unwrap();

    let installed = packages.installed().await.unwrap();
    let ids: Vec<_> = installed
        .iter()
        .map(|package| package.manifest.id.as_str())
        .collect();
    assert_eq!(ids, ["decl-ext", "luau-ext"]);
    assert!(
        installed
            .iter()
            .all(|package| package.enabled && package.origin_url == ORIGIN)
    );
    let runnable: Vec<_> = packages
        .runnable()
        .await
        .unwrap()
        .into_iter()
        .map(|source| source.id)
        .collect();
    assert_eq!(
        runnable,
        [ExtensionId::new("luau-ext")],
        "a declarative row is stored, not run yet"
    );

    let id = ExtensionId::new("luau-ext");
    packages.disable(&id, at(3)).await.unwrap();
    assert!(
        packages.runnable().await.unwrap().is_empty(),
        "a disabled extension doesn't run"
    );
    assert_eq!(
        packages.installed().await.unwrap().len(),
        2,
        "disable keeps the row"
    );
    packages.enable(&id, at(4)).await.unwrap();
    assert_eq!(packages.runnable().await.unwrap().len(), 1);

    packages.uninstall(&id).await.unwrap();
    assert_eq!(packages.installed().await.unwrap().len(), 1);
    assert!(matches!(
        packages.uninstall(&id).await,
        Err(PackageError::NotInstalled(_))
    ));
}

#[tokio::test]
async fn duplicates_bad_manifests_and_bad_origins_are_refused() {
    let (_dir, db) = db().await;
    let packages = Packages::new(&db);
    packages
        .install(&manifest("my-ext", "luau"), ORIGIN, at(1))
        .await
        .unwrap();
    let again = packages
        .install(&manifest("my-ext", "luau"), ORIGIN, at(2))
        .await;
    assert!(
        matches!(again, Err(PackageError::AlreadyInstalled(ref id)) if id.as_str() == "my-ext"),
        "{again:?}"
    );

    let bad = packages.install("{}", ORIGIN, at(3)).await.unwrap_err();
    assert!(matches!(bad, PackageError::Manifest(_)), "{bad:?}");
    let http = packages
        .install(
            &manifest("other", "luau"),
            "http://example.org/i.json",
            at(4),
        )
        .await
        .unwrap_err();
    assert_eq!(
        http.to_string(),
        "origin_url: must be an https URL: got http:"
    );

    assert_eq!(
        packages.installed().await.unwrap().len(),
        1,
        "failed installs left nothing"
    );
}
