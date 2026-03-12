# Compiling rusty-v8 for HermitOS (x86_64-unknown-hermit)

This document describes the changes needed to compile the V8 static library
(via rusty-v8) for the `x86_64-unknown-hermit` target, the issues encountered,
and the technical decisions made.

## Overview

HermitOS is a POSIX-compatible but minimalist unikernel. V8 and its build
system (GN/Ninja) don't support it natively. Three layers need to be patched:

| Layer | Repo | Role |
|-------|------|------|
| Chromium build system | `build/` (submodule) | BUILDCONFIG.gn — target OS declaration |
| V8 C++ | `v8/` (submodule) | OS detection, platform layer, POSIX guards |
| rusty-v8 | `build.rs` (root) | GN args, link flags, patch application |

## Architecture: patch strategy

Rather than forking the `v8/` and `build/` submodules, we use a
**patch strategy** (similar to Electron for Chromium):

```
patches/
├── v8/
│   └── 0001-add-hermitos-platform-support.patch
└── build/
    └── 0001-add-hermitos-as-supported-target-os.patch
```

Patches are applied automatically by `build.rs` when compiling for the `hermit`
target. Application is **idempotent**: if a patch is already applied (verified
via `git apply --reverse --check`), it is skipped.

Advantages:
- Submodules stay pointed at denoland (upstream)
- No forks to keep in sync during V8 updates
- Changes are explicit and versioned in `patches/`

Drawback:
- If upstream V8 modifies the same files, patches may fail to apply and will
  need to be regenerated

### Regenerating a patch

If a patch needs updating (e.g. after a V8 rebase):

```bash
# 1. Apply the current patch
cd v8
git apply ../patches/v8/0001-add-hermitos-platform-support.patch

# 2. Make the necessary adjustments
# ... edit files ...

# 3. Regenerate the patch
git diff > ../patches/v8/0001-add-hermitos-platform-support.patch

# 4. Clean up
git checkout .
```

## Prerequisites

- Rust nightly toolchain with the `x86_64-unknown-hermit` target:
  ```bash
  rustup target add x86_64-unknown-hermit
  ```
- **Clang 19+** with `libclang` — required for bindgen (V8 uses a recent
  libc++ that requires Clang 19+ builtins). On Ubuntu:
  ```bash
  # Add the LLVM repo if needed
  wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | sudo tee /etc/apt/trusted.gpg.d/apt.llvm.org.asc > /dev/null
  echo "deb https://apt.llvm.org/$(lsb_release -cs)/ llvm-toolchain-$(lsb_release -cs)-19 main" | sudo tee /etc/apt/sources.list.d/llvm-19.list
  sudo apt-get update && sudo apt-get install libclang-19-dev
  ```
- Python 3 (for GN)
- Ninja
- Git (for `git apply` of patches)

## Patch contents

### `build/` patch: `0001-add-hermitos-as-supported-target-os.patch`

GN doesn't know `hermit` as a valid OS. This patch modifies two files:

#### `config/BUILDCONFIG.gn` — Toolchain and `is_linux`

```gn
} else if (target_os == "hermit") {
  # HermitOS: use the Linux/Clang toolchain as base
  _default_toolchain = "//build/toolchain/linux:clang_$target_cpu"
```

And extends `is_linux` to include hermit:
```gn
is_linux = current_os == "linux" || current_os == "hermit"
```

We reuse the Linux toolchain because HermitOS uses the same ABI (System V
x86_64) and the same binary format (ELF). `is_linux` is needed so that the
`clang_lib` template (in `build/config/clang/BUILD.gn`) finds the clang
runtime libraries under `x86_64-unknown-linux-gnu/`.

#### `config/c++/modules.gni` — Module platform

Maps hermit to the `"linux"` platform for clang modules:
```gn
if (is_chromeos || current_os == "hermit") {
  module_platform = "linux"
```

Without this, GN would look for `build/modules/hermit/BUILD.gn` which doesn't
exist.

### `v8/` patch: `0001-add-hermitos-platform-support.patch`

This patch touches 5 files:

#### `include/v8config.h` — OS detection

Declares `V8_OS_HERMIT` and `V8_OS_POSIX`. Placed BEFORE the `__linux__`
block because HermitOS may also define `__linux__`.

#### `src/base/platform/platform-hermit.cc` — Platform layer (new file)

Inspired by `platform-aix.cc`. Implemented functions:
- `OS::CreateTimezoneCache()` -> `PosixDefaultTimezoneCache`
- `OS::SignalCodeMovingGC()` -> no-op
- `OS::AdjustSchedulingParams()` -> no-op
- `OS::GetSharedLibraryAddresses()` -> empty vector (no .so)
- `OS::GetFirstFreeMemoryRangeWithin()` -> `nullopt`
- `OS::RemapPages()` -> `false` (no `mremap`)
- `OS::DiscardSystemPages()` -> no-op (no `madvise`)
- `OS::DecommitPages()` -> `mmap(MAP_FIXED | MAP_ANONYMOUS | PROT_NONE)`

#### `BUILD.gn` — Source file registration

```gn
} else if (current_os == "hermit") {
  sources += [
    "src/base/debug/stack_trace_posix.cc",
    "src/base/platform/platform-hermit.cc",
  ]
}
```

Also adds `V8_HAVE_TARGET_OS` and `V8_TARGET_OS_LINUX` defines for hermit,
required for WebAssembly object size calculations in `std-object-sizes.h`.

#### `src/base/platform/platform-posix.cc` — Compilation guards

| Missing API | Solution |
|-------------|----------|
| `<sys/syscall.h>` | Exclude via `!V8_OS_HERMIT` |
| `DiscardSystemPages` (madvise) | `#if !V8_OS_HERMIT` |
| `DecommitPages` (mremap/madvise) | `#if !defined(_AIX) && !V8_OS_HERMIT` |
| `pthread_getattr_np` | `#elif V8_OS_HERMIT` -> return `nullptr` |

## Changes in `build.rs`

### Bindgen (Rust bindings generation)

HermitOS is treated like Linux for bindgen configuration:
- The clang resource directory is added (builtin headers like `stddef.h`)
- The multiarch path `/usr/include/x86_64-linux-gnu` is added because the
  host clang doesn't include it automatically for a cross-compilation target
  (needed for `bits/libc-header-start.h` referenced by `stdint.h`)

### ABI callbacks (`src/isolate.rs`)

HermitOS doesn't define `target_family` in Rust (neither `unix` nor `windows`).
ABI types for V8 callbacks (`RawHostImportModuleDynamicallyCallback`, etc.) use:
```rust
#[cfg(any(target_family = "unix", target_os = "hermit"))]
```

Since HermitOS uses the SysV ABI (like Unix), the Unix variant is correct.

### GN arguments

```rust
if target_os == "hermit" {
    gn_args.push(r#"target_os="hermit""#.to_string());
    gn_args.push("treat_warnings_as_errors=false".to_string());
    gn_args.push("v8_enable_trap_handler=false".to_string());
    gn_args.push("v8_enable_sandbox=false".to_string());
    gn_args.push("use_sysroot=false".to_string());
    gn_args.push("use_custom_libcxx=false".to_string());
    gn_args.push("enable_rust=false".to_string());
    gn_args.push("v8_enable_temporal_support=false".to_string());
}
```

Rationale:
- **`v8_enable_trap_handler=false`**: WASM is enabled but with explicit bounds
  checks instead of the signal-based trap handler (HermitOS doesn't have a
  reliable `sigaltstack`). Minor perf penalty on WASM memory accesses.
- **`v8_enable_sandbox=false`**: Requires 1 TB virtual memory reservation
- **`use_sysroot=false`**: No Chromium sysroot for Hermit
- **`use_custom_libcxx=false`**: Use the toolchain's libc++
- **`enable_rust=false`**: See section below
- **`v8_enable_temporal_support=false`**: Depends on `enable_rust`
- **`treat_warnings_as_errors=false`**: Incomplete POSIX headers

### C++ linking

HermitOS doesn't have a dynamic `libc++.so`:
```rust
} else if target.contains("hermit") {
    // HermitOS: no dynamic C++ stdlib to link
}
```

## Why `enable_rust=false` in GN

By default, GN enables `enable_rust=true`, which compiles the Rust stdlib for
Chromium's internal components (`libminiz_oxide`, etc.). Two problems:

### The adler vs adler2 issue

The stdlib renamed `adler` to `adler2` (Rust 1.79+). GN picks the name via
`rustc_nightly_capability`, but forces `false` when using an external
toolchain — even if it's nightly.

### The target triple issue

`build/config/rust.gni` maintains a whitelist of Rust triples.
`x86_64-unknown-hermit` is not in it.

### Solution

`enable_rust=false` in GN. Rust compilation is handled by Cargo; V8 is pure
C++. The Chromium components requiring Rust are not used by rusty-v8.

## Building

```bash
# From the rusty-v8 root
LIBCLANG_PATH=/usr/lib/llvm-19/lib V8_FROM_SOURCE=1 \
  cargo +nightly build --target x86_64-unknown-hermit -Zbuild-std=std,panic_abort

# With GN debug output
LIBCLANG_PATH=/usr/lib/llvm-19/lib V8_FROM_SOURCE=1 PRINT_GN_ARGS=1 \
  cargo +nightly build --target x86_64-unknown-hermit -Zbuild-std=std,panic_abort
```

Notes:
- **`LIBCLANG_PATH`**: required, must point to Clang 19+
- **`-Zbuild-std=std,panic_abort`**: hermit has no prebuilt std, `panic_abort`
  must be included explicitly
- **`+nightly`**: required for `-Zbuild-std`

## Limitations

- **WebAssembly**: enabled with explicit bounds checks (no trap handler)
- **V8 Sandbox**: disabled (insufficient virtual memory reservation)
- **Profiling / stack traces**: non-functional
- **Pointer compression**: should work (x86_64)
- **Snapshots**: should work (no OS dependency)
- **Temporal API**: disabled (depends on enable_rust)
