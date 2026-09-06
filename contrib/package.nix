{
  lib,
  stdenv,
  rustPlatform,
  llvmPackages,
  pkg-config,
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

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.fileFilter ({ hasExt, ... }: !hasExt "nix") ../.;
  };

  cargoLock = {
    lockFile = ../Cargo.lock;
  };

  nativeBuildInputs = [
    pkg-config
    rustPlatform.bindgenHook
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
  '';

  meta = {
    description = "Rewrite of ananicy in Rust for lower CPU and memory usage";
    homepage = "https://github.com/JohnRTitor/ananicy-rs";
    license = lib.licenses.gpl3Only;
    platforms = lib.platforms.linux;
    mainProgram = "ananicy-rs";
  };
}
