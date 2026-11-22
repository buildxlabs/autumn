{
  self,
  nixpkgs,
  crucible,
  rust-manifest,
  ...
}:
let
  systems = import ./lib/systems.nix {
    inherit (nixpkgs) lib;
    inherit nixpkgs;
  };

  inherit (systems) forEachSupportedSystem;

  # Shared per-system inputs, so no output file has to re-derive them.
  perSystem =
    f:
    forEachSupportedSystem (
      { pkgs, system }:
      let
        toolchains = import ./lib/toolchains.nix {
          inherit pkgs crucible rust-manifest;
        };
        nodeToolchain = toolchains.nodeToolchain;
        repoRoot = self;
      in
      f {
        inherit
          pkgs
          system
          toolchains
          nodeToolchain
          repoRoot
          ;
      }
    );
in
{
  formatter = forEachSupportedSystem ({ pkgs, ... }: pkgs.nixfmt);

  overlays.default =
    final: prev:
    let
      toolchains = import ./lib/toolchains.nix {
        pkgs = prev;
        inherit crucible rust-manifest;
      };
    in
    {
      inherit (toolchains) rustToolchain nodeToolchain;
      pnpm = toolchains.nodeToolchain.packageManager;
      vite-plus = toolchains.nodeToolchain.vite-plus;
    };

  devShells = perSystem (
    {
      pkgs,
      toolchains,
      nodeToolchain,
      ...
    }:
    {
      default = import ./devshells/default.nix {
        inherit pkgs nodeToolchain;
        # The channel's own cargo, not the huacnlee fork: the fork forces one
        # build directory under ~/.cargo/targets for every worktree, and
        # parallel builds in separate worktrees deadlocked on its locks. Stock
        # cargo builds into each worktree's own target/.
        rustToolchain = toolchains.rustToolchain.stock;
      };
    }
  );

  packages = perSystem (
    { toolchains, nodeToolchain, ... }:
    {
      default = toolchains.rustToolchain;
      rust = toolchains.rustToolchain;
      pnpm = nodeToolchain.packageManager;
      vite-plus = nodeToolchain.vite-plus;
    }
  );
}
