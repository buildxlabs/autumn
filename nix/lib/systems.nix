# The systems this flake produces outputs for.
{ lib, nixpkgs }:
let
  supportedSystems = [
    "x86_64-linux"
    "aarch64-linux"
    "aarch64-darwin"
  ];

  forEachSupportedSystem =
    f:
    lib.genAttrs supportedSystems (
      system:
      f {
        inherit system;
        pkgs = import nixpkgs {
          inherit system;
          config.allowUnfree = true;
        };
      }
    );
in
{
  inherit supportedSystems forEachSupportedSystem;
}
