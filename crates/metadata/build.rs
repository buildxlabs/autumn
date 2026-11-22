//! Registers AniList's schema so the `cynic` derives can check our queries.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=schema/anilist.graphql");
    cynic_codegen::register_schema("anilist")
        .from_sdl_file("schema/anilist.graphql")?
        .as_default()?;
    Ok(())
}
