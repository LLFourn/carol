{
  description = "Carol";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane, advisory-db, ... }:
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          rustupToolchainToml = (readTomlFile ./rust-toolchain.toml).toolchain;
          overlays = [
            # use rust overlay to provide a rust toolchain same as rustup would
            (import rust-overlay)
            (self: super: {
              rustToolchain = (super.rust-bin.fromRustupToolchain ({
                channel = "1.81.0";
              } // rustupToolchainToml));

              rustToolchain-nightly = (super.rust-bin.fromRustupToolchain ({
                channel = "nightly";
              } // rustupToolchainToml));
            })
          ];

          pkgs = import nixpkgs {
            inherit system overlays;
          };

          craneLib = crane.mkLib pkgs;

          src = pkgs.lib.cleanSourceWith {
            src = craneLib.path ./.;
            filter = path: type:
              (pkgs.lib.hasSuffix "\.wit" path) ||
              (craneLib.filterCargoSources path type);
          };

          # parse the list of workspace members, separating the example crates
          # out from the packages we want to output in the flake packages
          readTomlFile = path: builtins.fromTOML (builtins.readFile path);
          workspaceMembers = (readTomlFile ./Cargo.toml).workspace.members;
          isExample = p: (builtins.substring 0 15 p) == "example-guests/";
          exportedMembers = builtins.filter (x: !isExample x) workspaceMembers;
          exampleMembers = builtins.filter isExample workspaceMembers;

          packageMetadata = crateDir: craneLib.crateNameFromCargoToml {
            cargoToml = ./. + "/${crateDir}/Cargo.toml";
          };

          carolCrateMetadata = packageMetadata "crates/carol";
          exportedPackageMetadata = builtins.map packageMetadata exportedMembers;
          examplePackageMetadata = builtins.map packageMetadata exampleMembers;

          # derivation arguments for skipping normal binary installation
          # used for checks with no build outputs or build artifact caching
          # derivations which are not meant to package what is built
          skipInstallArgs = {
            installPhaseCommand = "";
            dontFixup = true;
          };

          # define an unexported package for caching the workspace build and its
          # dependencies, which will get reused in subsequent derivations
          cargoArtifacts =
            let
              pname = "carol-workspace";
            in
            craneLib.buildPackage (with carolCrateMetadata; skipInstallArgs // {
              # first, cache just the dependencies
              cargoArtifacts =
                craneLib.buildDepsOnly (with carolCrateMetadata; skipInstallArgs // {
                  inherit pname version src;
                });

              # next, build the workspace normally (no -p args to cargo build)
              inherit pname version src;
              pnameSuffix = "-artifacts";
              doInstallCargoArtifacts = true;
              doCheck = false; # don't fail just yet
            });

          # finally build again, one package at a time. this prevents parallel
          # re-compilation of the per-crate dependencies in the flake checks,
          # but doesn't really avoid the underlying cost of multiple compiler
          # invocations.
          #
          # quoting from https://doc.rust-lang.org/cargo/reference/resolver.html#features
          # > When building multiple packages in a workspace (such as with --workspace or
          # > multiple -p flags), the features of the dependencies of all of those
          # > packages are unified. If you have a circumstance where you want to avoid
          # > that unification for different workspace members, you will need to build
          # > them via separate cargo invocations.
          exhaustiveCargoArtifacts = craneLib.buildPackage (with carolCrateMetadata; skipInstallArgs // {
            inherit version src cargoArtifacts;
            pname = "carol-workspace";
            pnameSuffix = "-exhaustive-artifacts-hack";
            doCheck = false;
            doInstallCargoArtifacts = true;
            buildPhaseCargoCommand = ''
              for pkg in ${builtins.toString (builtins.map (p: p.pname) (exportedPackageMetadata ++ examplePackageMetadata))}; do
                cargoWithProfile build -p $pkg
              done
            '';
          });

          # helper function for providing a package definition given a
          # Cargo.toml in the workspace
          buildWorkspacePackage = cargoMeta:
            craneLib.buildPackage (cargoMeta // {
              inherit src;
              cargoArtifacts = exhaustiveCargoArtifacts;
              doCheck = false; # tests are run in nix flake check with cargo nextest
              cargoExtraArgs = "-p ${cargoMeta.pname}";
            });

          # these are nix packages, as opposed to crate packages
          exportedPackages = builtins.map buildWorkspacePackage exportedPackageMetadata;
          examplePackages = builtins.map buildWorkspacePackage examplePackageMetadata;

          # helper function to convert a list of packages to an attr set
          packagesToAttrs = list: builtins.listToAttrs (builtins.map
            (p: {
              name = p.pname;
              value = p;
            })
            list);

          carolToolchain = pkgs.buildEnv {
            name = "carol-toolchain";
            paths = with (packagesToAttrs exportedPackages); [ carol carlo ];
          };
          carolCrates = pkgs.buildEnv {
            name = "carol-crates";
            paths = exportedPackages;
          };
          examples = pkgs.buildEnv {
            name = "carol-examples";
            paths = examplePackages;
          };

          clippy-sarif = pkgs.rustPlatform.buildRustPackage rec {
            pname = "clippy-sarif";
            version = "0.3.7";
            cargoHash = "sha256-6lfpLyUGJ4J6WPjD1QR1UczEy0qTDecqblN6HVdDaJo=";
            src = pkgs.fetchCrate {
              inherit pname version;
              hash = "sha256-6QAaHs1D3xnVEQI3+U/a9zmjYUMzGgElGI93GD5Edtk=";
            };
          };

          sarif-fmt = pkgs.rustPlatform.buildRustPackage rec {
            pname = "sarif-fmt";
            version = "0.3.7";
            cargoHash = "sha256-ToLWGMt9II6C+O7VbJZ7wC01C/0yAg4aAFM/V7piHWo=";
            doCheck = false; # buildInputs = [ clang-tools ]; # needs clang-tidy in test
            src = pkgs.fetchCrate {
              inherit pname version;
              hash = "sha256-On9qis+I+p2wZCoxX4S2uu9DXdh+zCgyIYctT6DedfI=";
            };
          };

          # additional nixpkgs supplied tools for devshells
          devShellPackages = with pkgs; [
              lldb
              cargo-nextest
              wasm-tools
              nix-output-monitor
              cachix
          ];

          devShellToolchainExtensions = [ "rust-src" "rust-analyzer" ];
        in
        with pkgs;
        {
          packages = (packagesToAttrs exportedPackages) // {
            default = carolToolchain;
            examples = examples // (packagesToAttrs examplePackages);
          };

          devShells = rec {
            default = stable;

            rustup = mkShell {
              buildInputs = [ pkgs.rustup ] ++ devShellPackages;
            };

            stable = mkShell {
              buildInputs = [
                (rustToolchain.override {extensions = devShellToolchainExtensions;})
              ] ++ devShellPackages;
            };

            nightly = mkShell {
              buildInputs = [
                (rustToolchain-nightly.override {extensions = devShellToolchainExtensions;})
              ] ++ devShellPackages;
            };
          };

          checks =
            let
              commonArgs = with carolCrateMetadata; {
                inherit pname version cargoArtifacts;
                src = ./.; # bypass Crane's source cleaning for checks

                # Nerf some unnecessary stdenv install things to reduce noise,
                # see https://ryantm.github.io/nixpkgs/stdenv/stdenv/
                installPhase = "touch $out";
                dontFixup = true;
              };
            in
            {
              # Build the crate as part of `nix flake check` for convenience
              inherit carolToolchain;
              inherit carolCrates;
              inherit examples;

              # Check that shellcheck approves of the README shell blocks and
              # that they execute to the best of our ability in the nix sandbox.
              # This requires mkCargoDerivation for cargo metadata, used by
              # carlo build, to work in the sandbox.
              readme = craneLib.mkCargoDerivation (commonArgs // {
                pname = "readme-codeblocks";
                version = carolCrateMetadata.version;
                buildInputs = [
                  carolToolchain
                  pkgs.rustToolchain # the wasm toolchain for carlo
                  pkgs.curl
                  pkgs.wasm-tools
                  # extract sh code blocks from markdown using a pandoc filter
                  (writeScriptBin "extract-codeblocks" ''
                    ${pkgs.pandoc}/bin/pandoc \
                      -f gfm -t native -o /dev/null \
                      --lua-filter /dev/stdin \
                      $* <<PANDOC_FILTER | ${pkgs.perl}/bin/perl -pe 's/cargo run -p (carlo|carol) --/$1/; s/curl/curl --fail-with-body/'
                    function CodeBlock (x)
                      if x.attr.classes[ 1 ] == "sh" then
                        print (x.text)
                      end
                    end
                    PANDOC_FILTER
                  '')
                ];
                buildPhaseCargoCommand = "extract-codeblocks README.md > README-codeblocks.sh";
                checkPhaseCargoCommand = ''
                  ${pkgs.shellcheck}/bin/shellcheck -s sh -o all README-codeblocks.sh

                  # likewise, the nix sandbox won't let us actually talk to
                  # bitmex, so we can't execute the activation
                  (
                    grep -v 'curl.*activate' README-codeblocks.sh
                    echo 'set +e; kill $( jobs -p )'
                  ) | ${pkgs.bash}/bin/sh -x -e
                '';
                doCheck = true;
              });

              nextest = craneLib.cargoNextest (commonArgs // {
                partitions = 1;
                partitionType = "count";
                installPhase = "cp -r target/nextest $out";
              });

              clippy = craneLib.cargoClippy (commonArgs // {
                # FIXME this abuses crane implementation detail to pipe things throgh clippy-sarif & sarif-fmt
                cargoClippyExtraArgs = "--no-deps --all-features --all-targets --message-format=json -- --deny warnings | clippy-sarif | tee clippy.sarif | sarif-fmt";

                nativeBuildInputs = [
                  clippy-sarif
                  sarif-fmt
                ];
                installPhase = "cp -r clippy.sarif $out";
              });

              doc = craneLib.cargoDoc (commonArgs // {
                installPhase = "cp -r target/doc $out";
              });

              fmt = craneLib.cargoFmt commonArgs;

              audit = craneLib.cargoAudit (commonArgs // {
                inherit advisory-db;
              });

              machete = craneLib.mkCargoDerivation (commonArgs // {
                pname = "carol-machete";
                nativeBuildInputs = [ cargo-machete ];
                buildPhaseCargoCommand = "";
                checkPhaseCargoCommand = "cargo machete";
                doCheck = true;
              });
            };
        }
      );
}
