#!/usr/bin/env bash
# Build the C API, compile examples/snap.c against the shared library (as C99, with
# warnings as errors), and run it on the simulated camera. Also checks the header
# compiles as C++. Works on Linux, macOS and Windows (Git Bash, with clang).
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release -p loupecam-capi
target=$(cargo metadata --format-version 1 --no-deps | grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4 | sed 's#\\\\#/#g')
lib="$target/release"
out="$target/capi-test"
mkdir -p "$out"
flags=(-std=c99 -Wall -Wextra -Werror -Iinclude)

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    cc=${CC:-clang}
    cxx=${CXX:-clang++}
    "$cc" "${flags[@]}" examples/snap.c "$lib/loupecam_c.dll.lib" -o "$out/snap.exe"
    cp "$lib/loupecam_c.dll" "$out/"
    exe="$out/snap.exe"
    ;;
  *)
    cc=${CC:-cc}
    cxx=${CXX:-c++}
    "$cc" "${flags[@]}" examples/snap.c -L"$lib" -lloupecam_c -Wl,-rpath,"$lib" -o "$out/snap"
    exe="$out/snap"
    ;;
esac

printf '#include "loupecam.h"\nint main() { return loupecam_version() == nullptr; }\n' |
  "$cxx" -std=c++17 -Wall -Wextra -Werror -Iinclude -x c++ -fsyntax-only -

"$exe" --simulated "$out/snap.png"
test -s "$out/snap.png"
