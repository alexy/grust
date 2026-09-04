# C++ Toolchain Notes

## macOS 26 (Tahoe) SDK: `<algorithm>` not found

On macOS 26 (Tahoe), the Command Line Tools install the full libc++ headers
inside the SDK at:

```
/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk/usr/include/c++/v1/
```

but the CLT's top-level include directory only contains a handful of stubs:

```
/Library/Developer/CommandLineTools/usr/include/c++/v1/   ← only stubs
```

Apple clang with `-mmacosx-version-min=26.x` does not automatically add the
SDK's C++ include path to the search list, so any crate that compiles C++
source fails with:

```
fatal error: 'algorithm' file not found
    2 | #include <algorithm>
error: failed to run custom build command for `cxx v1.0.138`
```

This affects `grust-ladybug` (which depends on `lbug` → `cxx`) and any other
crate that pulls in the `cxx` bridge crate.

### Fix

Use the SDK selected by the active Apple developer toolchain. Do not commit an
absolute SDK path to `.cargo/config.toml`: Xcode and Command Line Tools can be
installed side by side, and mixing headers from the inactive installation
causes missing `intmax_t`, `uint*_t`, and `_CTYPE_*` declarations.

Most current Xcode installations require no override. If a C++ bridge still
cannot find libc++, derive the include path for that shell invocation:

```sh
active_sdk="$(xcrun --show-sdk-path)"
CXXFLAGS="-isystem ${active_sdk}/usr/include/c++/v1" cargo build --workspace --all-features
```

### cargo publish --verify

`cargo publish --verify` spawns a clean build in a temporary directory and
inherits the shell environment. Apply the dynamic fallback to the publish
invocation only if the default toolchain lookup fails. `grust-ladybug` remains
`publish = false`; published Grust crates must never bypass verification.

### Diagnose the selected SDK

```sh
xcode-select -p
xcrun --show-sdk-path
clang++ --version
```

If those commands point at different installations, select the intended
developer directory before retrying. Keep that machine-specific choice out of
the repository.
