{
  pkgs,
  crucible,
  rust-manifest,
}:
let
  rustToolchain = crucible.lib.${pkgs.stdenv.hostPlatform.system}.rust.fromToolchainFile {
    file = ../../rust-toolchain.toml;
    manifest = rust-manifest;
  };

  nodeToolchain = crucible.lib.${pkgs.stdenv.hostPlatform.system}.node.fromPackageJson {
    file = ../../package.json;
  };
in
{
  inherit rustToolchain nodeToolchain;
}
