{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      crane,
      rust-overlay,
      ...
    }:
    let
      forEachSystem =
        f:
        nixpkgs.lib.genAttrs
          [
            "aarch64-darwin"
            "x86_64-linux"
            "aarch64-linux"
            "x86_64-darwin"
          ]
          (
            system:
            f {
              pkgs = import nixpkgs {
                inherit system;
                overlays = [ rust-overlay.overlays.default ];
              };
              inherit system;
            }
          );
    in
    {
      packages = forEachSystem (
        { pkgs, system }:
        let
          toolchain = pkgs.rust-bin.stable.latest.default;
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;

          # cleanCargoSource removes the non-Rust files. The bundled docs
          # and the missouri fixtures must survive the filter.
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter =
              path: type:
              (craneLib.filterCargoSources path type)
              || (builtins.match ".*tests/missouri/.*" path != null)
              || (builtins.match ".*\\.yml$" path != null)
              || (builtins.match ".*\\.missouri.*" path != null)
              || (builtins.match ".*/docs$" path != null)
              || (builtins.match ".*/docs/.*" path != null)
              || (builtins.match ".*/skills$" path != null)
              || (builtins.match ".*/skills/.*" path != null);
          };

          commonArgs = {
            pname = "gaff";
            version = "0.1.0";
            inherit src;
            strictDeps = true;
            buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
            ];
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          gaff = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              # The CLI is parsed by hand (the never-exit-2 rule keeps
              # clap out), so the man page is hand-written in docs/man/
              # and installed here.
              postInstall = ''
                mkdir -p $out/share/man/man1
                cp docs/man/gaff.1 $out/share/man/man1/
              '';
              # The integration tests drive a real git to build their
              # fixtures, and stdenv builds PATH from the declared inputs
              # alone. Sandboxing is not the mechanism: the build fails
              # the same way with sandbox = false. It has to be a native
              # check input, because strictDeps keeps host-platform
              # inputs off PATH, so checkInputs reaches nothing and the
              # same eight tests fail. gitMinimal carries every builtin
              # the fixtures use, at a tenth of the closure.
              nativeCheckInputs = [ pkgs.gitMinimal ];
              # The nix check runs cargo test, so the unit tests and the
              # integration tests both run here. The missouri suite is not
              # a cargo target at all: tests/missouri holds state
              # directories and no .rs file, so cargo never builds it. CI
              # installs missouri through its own action.
              checkPhase = ''
                tmpHome="$(mktemp -d)"
                export HOME="$tmpHome"
                cargo test --profile release --locked
              '';
            }
          );
        in
        {
          default = gaff;
          inherit gaff;
        }
      );

      devShells = forEachSystem (
        { pkgs, ... }:
        {
          default = pkgs.mkShell {
            buildInputs = [
              pkgs.rust-bin.stable.latest.default
              pkgs.jq
              # `cargo test` here runs the review-note fixtures, which drive
              # git. An impure shell inherits the user's git and passes; a
              # pure one has none and fails the same eight tests.
              pkgs.gitMinimal
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
            ];
          };
        }
      );
    };
}
