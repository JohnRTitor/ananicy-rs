{
  description = "ananicy-rs: Ananicy rewrite in Rust";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs =
    inputs@{ self, ... }:
    let
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

      rev =
        self.shortRev or self.dirtyShortRev or (inputs.nixpkgs.lib.substring 0 8 self.lastModifiedDate);
      version = "${cargoToml.workspace.package.version}+${rev}";
    in
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      flake.nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        {
          imports = [ ./contrib/module.nix ];
          services.ananicy-rs.package =
            lib.mkDefault
              self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        };

      perSystem =
        {
          config,
          pkgs,
          system,
          ...
        }:
        {
          formatter = pkgs.nixfmt-rfc-style;

          packages = {
            default = pkgs.callPackage ./contrib/package.nix { inherit version; };
            ananicy-rs = config.packages.default;
          };

          checks = {
            ananicy-rs = config.packages.default;
          };

          devShells.default = pkgs.mkShell {
            inputsFrom = [ config.packages.default ];
            buildInputs = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              # Include clang so that BPF compilation works outside of Nix build
              libbpf
              llvmPackages.clang-unwrapped
            ];
            hardeningDisable = [ "zerocallusedregs" ];
          };
        };
    };
}
