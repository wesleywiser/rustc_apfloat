use std::process::{Command, ExitCode};

// HACK(eddyb) should avoid shelling out, but for now this will suffice.
const SH_SCRIPT: &str = r#"
set -e

llvm_project_tgz_url="https://codeload.github.com/llvm/llvm-project/tar.gz/$llvm_project_git_hash"
curl -sS "$llvm_project_tgz_url" | tar -C "$OUT_DIR" -xz
llvm="$OUT_DIR"/llvm-project-"$llvm_project_git_hash"/llvm

mkdir -p "$OUT_DIR"/fake-config/llvm/Config
touch "$OUT_DIR"/fake-config/llvm/Config/{abi-breaking,llvm-config}.h

# HACK(eddyb) bypass `$llvm/include/llvm/Support/DataTypes.h.cmake`.
mkdir -p "$OUT_DIR"/fake-config/llvm/Support
echo -e '#include <'{math,inttypes,stdint,sys/types}'.h>\n' \
  > "$OUT_DIR"/fake-config/llvm/Support/DataTypes.h

# FIXME(eddyb) maybe split `$clang_codegen_flags` into front-end vs back-end.
clang_codegen_flags="-fPIC -fno-exceptions -O3 -march=native"

# HACK(eddyb) first compile all the source files into one `.bc` file:
# - "unity build" (w/ `--include`) lets `-o` specify path (no `--out-dir` sadly)
# - LLVM `.bc` intermediate allows the steps below to reduce dependencies
echo | clang++ -x c++ - -std=c++17 \
  $clang_codegen_flags \
  -I "$llvm"/include \
  -I "$OUT_DIR"/fake-config \
  -DNDEBUG \
  --include="$llvm"/lib/Support/{APInt,APFloat}.cpp \
  --include=cxx_apf_fuzz.cc \
  -c -emit-llvm -o "$OUT_DIR"/cxx_apf_fuzz.bc

# HACK(eddyb) use the `internalize` pass (+ O3) to prune everything unexported.
# FIXME(eddyb) this was just the above hack, but had to move sancov here, to
# replicate https://github.com/rust-fuzz/afl.rs/blob/8ece4f9/src/bin/cargo-afl.rs#L370-L372
# *after* `internalize` & optimizations (to avoid instrumenting dead code).
opt \
  --internalize-public-api-list="$cxx_apf_fuzz_exports" \
  --passes='internalize,default<O3>,sancov-module' \
  --sanitizer-coverage-level=3 \
  --sanitizer-coverage-trace-pc-guard \
  --sanitizer-coverage-prune-blocks=0 \
  "$OUT_DIR"/cxx_apf_fuzz.bc \
  -o "$OUT_DIR"/cxx_apf_fuzz.opt.bc

# HACK(eddyb) let Clang do the rest of the work, from the pruned `.bc`.
# FIXME(eddyb) maybe split `$clang_codegen_flags` into front-end vs back-end.
clang++ $clang_codegen_flags \
  "$OUT_DIR"/cxx_apf_fuzz.opt.bc \
  -c -o "$OUT_DIR"/cxx_apf_fuzz.o

llvm-ar rc "$OUT_DIR"/libcxx_apf_fuzz.a "$OUT_DIR"/cxx_apf_fuzz.o

echo cargo:rerun-if-changed=cxx_apf_fuzz.cc
echo cargo:rustc-link-lib="$OUT_DIR"/libcxx_apf_fuzz.a
echo cargo:rustc-link-lib=stdc++
"#;

fn main() -> std::io::Result<ExitCode> {
    // HACK(eddyb) work around https://github.com/rust-lang/cargo/issues/3676,
    // by removing the env vars that Cargo appears to hardcode.
    const CARGO_HARDCODED_ENV_VARS: &[(&str, &str)] = &[
        ("SSL_CERT_DIR", "/etc/pki/tls/certs"),
        ("SSL_CERT_FILE", "/etc/pki/tls/certs/ca-bundle.crt"),
    ];
    for &(var_name, cargo_hardcoded_value) in CARGO_HARDCODED_ENV_VARS {
        if let Ok(value) = std::env::var(var_name) {
            if value == cargo_hardcoded_value {
                std::env::remove_var(var_name);
            }
        }
    }

    let sh_script_exit_status = Command::new("sh")
        .args(["-c", SH_SCRIPT])
        .envs([
            // FIXME(eddyb) ensure this is kept in sync.
            (
                "llvm_project_git_hash",
                "f3598e8fca83ccfb11f58ec7957c229e349765e3",
            ),
            (
                "cxx_apf_fuzz_exports",
                "cxx_apf_fuzz_eval_req_ieee32,cxx_apf_fuzz_eval_req_ieee64",
            ),
        ])
        .status()?;
    Ok(if sh_script_exit_status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
