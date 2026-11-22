//! Rebuilds `db` when a migration is added. `sqlx::migrate!` embeds the
//! directory's contents, but cargo only watches files it already knows about,
//! so a new `.sql` file alone wouldn't trigger a recompile.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
