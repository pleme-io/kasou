{
  description = "Kasou (仮想) — safe Apple Virtualization.framework bindings for macOS VM management";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      substrate,
      ...
    }:
    let
      system = "aarch64-darwin";
      pkgs = import nixpkgs { inherit system; };

      props = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      version = props.package.version;
      pname = "kasou";

      darwinInputs =
        if pkgs ? apple-sdk then
          [ pkgs.apple-sdk ]
        else
          pkgs.lib.optionals (pkgs ? darwin) (
            with pkgs.darwin.apple_sdk.frameworks;
            [
              Security
              SystemConfiguration
            ]
          );

      package = pkgs.rustPlatform.buildRustPackage {
        inherit pname version;
        src = pkgs.lib.cleanSource ./.;
        cargoLock.lockFile = ./Cargo.lock;
        buildInputs = darwinInputs;
        doCheck = true;
        meta = {
          description = props.package.description;
          homepage = props.package.homepage;
          license = pkgs.lib.licenses.mit;
        };
      };
    in
    {
      packages.${system} = {
        kasou = package;
        default = package;
      };

      overlays.default = final: prev: {
        kasou = self.packages.${final.system}.default;
      };

      devShells.${system}.default = pkgs.mkShellNoCC {
        packages = [
          pkgs.rustc
          pkgs.cargo
          pkgs.rust-analyzer
        ] ++ darwinInputs;
      };

      formatter.${system} = pkgs.nixfmt-tree;
    };
}
