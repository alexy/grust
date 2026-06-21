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

`.cargo/config.toml` at the workspace root adds the SDK's C++ path via
`CXXFLAGS`:

```toml
[env]
CXXFLAGS = "-isystem /Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk/usr/include/c++/v1"
```

This is already committed. The `[env]` table only applies when the variable is
not already set in the shell, so it does not interfere with CI or Docker
environments that supply their own `CXXFLAGS`.

### cargo publish --verify

`cargo publish --verify` spawns a clean build in a temp directory and inherits
the shell environment. The `.cargo/config.toml` fix applies there, so
`grust-ladybug` can now be published without `--no-verify`. Before this fix,
the workaround was `cargo publish -p grust-ladybug --no-verify`.

### If the SDK version changes

The path above is pinned to `MacOSX26.5.sdk`. If a newer Tahoe seed updates
the SDK to `MacOSX26.6.sdk` or similar, update the path in
`.cargo/config.toml`. Run `ls /Library/Developer/CommandLineTools/SDKs/` to
see which SDKs are installed.
