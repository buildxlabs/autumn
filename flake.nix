{
  description = "Autumn: Rust workspace with pnpm, Vite+ and Markdown tooling";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    crucible = {
      url = "github:kiyors/crucible";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Must match `channel` in rust-toolchain.toml. `nix flake update` moves to
    # the newest patch release of the 1.100 series without a hash to paste.
    rust-manifest = {
      url = "https://static.rust-lang.org/dist/channel-rust-1.100.toml";
      flake = false;
    };
  };

  outputs = inputs: import ./nix inputs;
}
