use {
    libbpf_cargo::SkeletonBuilder,
    std::{
        env,
        path::{Path, PathBuf},
    },
};

fn get_bpf_arch(target_arch: &str) -> &'static str {
    match target_arch {
        "x86_64" => "x86",
        "aarch64" => "arm64",
        "loongarch64" => "loongarch",
        "riscv64" => "riscv",
        _ => panic!("Unsupported architecture for BPF: {}", target_arch),
    }
}

fn main() {
    let mut out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
    out.push("ananicy_cpp.skel.rs");

    let bpf_src = Path::new("bpf/ananicy_cpp.bpf.c");
    let bpf_include = Path::new("bpf/include");

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH not set");
    let arch_include = Path::new("bpf").join(get_bpf_arch(&target_arch));

    println!("cargo:rerun-if-changed=bpf/");

    let mut clang_args = vec![
        format!("-I{}", bpf_include.display()),
        format!("-I{}", arch_include.display()),
    ];

    if let Ok(libbpf) = pkg_config::Config::new().probe("libbpf") {
        for path in libbpf.include_paths {
            clang_args.push(format!("-I{}", path.display()));
        }
    }

    SkeletonBuilder::new()
        .source(bpf_src)
        .clang_args(clang_args.iter().map(String::as_str))
        .build_and_generate(&out)
        .expect("Failed to build BPF skeleton. Do you have clang installed?");
}
