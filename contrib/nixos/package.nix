{
  lib,
  stdenv,
  rustPlatform,
  llvmPackages,
  pkg-config,
  installShellFiles,
  elfutils,
  zlib,
  zstd,
  libbpf,
  pcre2,
  systemdLibs,
  version ? "unstable",
  withBpf ? true,
  withSystemd ? lib.meta.availableOn stdenv.hostPlatform systemdLibs,
}:
rustPlatform.buildRustPackage {
  pname = "ananicy-rs";
  inherit version;

  strictDeps = true;
  __structuredAttrs = true;

  # Only the tree the daemon is built from. `crates/ananicy-platform/fuzz` is
  # its own cargo workspace and is never built here, and dropping every `.nix`
  # file keeps this expression from vendoring itself.
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.fileFilter ({ hasExt, ... }: !hasExt "nix") ../..;
  };

  cargoLock = {
    lockFile = ../../Cargo.lock;
  };

  nativeBuildInputs = [
    pkg-config
    rustPlatform.bindgenHook
    installShellFiles
  ]
  ++ lib.optionals withBpf [
    llvmPackages.clang
  ];

  buildInputs = [
    pcre2
  ]
  ++ lib.optionals withBpf [
    elfutils
    zlib
    zstd
    libbpf
  ]
  ++ lib.optionals withSystemd [
    systemdLibs
  ];

  buildNoDefaultFeatures = true;
  buildFeatures = [ "netlink" ]
    ++ lib.optionals withBpf [ "bpf" ]
    ++ lib.optionals withSystemd [ "systemd" ];

  checkFlags = [
    # Fails in Nix sandbox due to restricted permissions
    "--skip=test_set_affinity_on_current_process"
  ];

  hardeningDisable = [
    "zerocallusedregs"
  ];

  postInstall = ''
    rm -rf $out/bin
    make install DESTDIR= PREFIX=$out CARGO_TARGET_DIR=target/${stdenv.hostPlatform.rust.cargoShortTarget}
    install -Dm644 -t $out/share/doc/$pname README.md CONTRIBUTING.md
    install -Dm644 -t $out/share/doc/$pname/docs docs/BUILD.md docs/CLI.md \
      docs/COMPATIBILITY.md docs/CONFIGURATION.md docs/SYSTEMD.md \
      docs/TESTING.md docs/TOPOLOGY.md
  '' + lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
    installShellCompletion --cmd ananicy-rs \
      --bash <($out/bin/ananicy-rs completions bash) \
      --fish <($out/bin/ananicy-rs completions fish) \
      --zsh <($out/bin/ananicy-rs completions zsh)
  '';

  meta = {
    description = "Rewrite of ananicy in Rust for lower CPU and memory usage";
    homepage = "https://github.com/JohnRTitor/ananicy-rs";
    license = lib.licenses.gpl3Only;
    platforms = lib.platforms.linux;
    mainProgram = "ananicy-rs";
  };
}
