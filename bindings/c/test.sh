#!/usr/bin/env bash
# Build the C API, compile examples/snap.c against it (as C99, warnings as errors),
# linked both dynamically and statically, and run each on the simulated camera. Also
# checks the header compiles as C++. Works on Linux, macOS and Windows (Git Bash, with
# clang).
set -euo pipefail
cd "$(dirname "$0")"

# Both library kinds, and the system libraries a static link needs.
libs=$(cargo rustc --release -p loupecam-capi --crate-type cdylib,staticlib -- --print native-static-libs 2>&1 |
  tee /dev/stderr | sed -n 's/.*native-static-libs: //p' | head -1)
target=$(cargo metadata --format-version 1 --no-deps | grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4 | sed 's#\\\\#/#g')
lib="$target/release"
out="$target/capi-test"
mkdir -p "$out"
flags=(-std=c99 -Wall -Wextra -Werror -Iinclude)

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    cc=${CC:-clang}
    cxx=${CXX:-clang++}
    exe=.exe
    cp "$lib/loupecam_c.dll" "$out/"
    "$cc" "${flags[@]}" examples/snap.c "$lib/loupecam_c.dll.lib" -o "$out/snap$exe"
    # As clang options: foo.lib is -lfoo, and MSVC linker options (/defaultlib:…) go.
    read -r -a system <<< "$(tr ' ' '\n' <<< "$libs" | sed -n 's/^\(.*\)\.lib$/-l\1/p' | tr '\n' ' ')"
    "$cc" "${flags[@]}" examples/snap.c "$lib/loupecam_c.lib" "${system[@]}" -o "$out/snap-static$exe"
    ;;
  *)
    cc=${CC:-cc}
    cxx=${CXX:-c++}
    exe=
    "$cc" "${flags[@]}" examples/snap.c -L"$lib" -lloupecam_c -Wl,-rpath,"$lib" -o "$out/snap"
    read -r -a system <<< "$libs"
    "$cc" "${flags[@]}" examples/snap.c "$lib/libloupecam_c.a" "${system[@]}" -o "$out/snap-static"
    ;;
esac

printf '#include "loupecam.h"\nint main() { return loupecam_version() == nullptr; }\n' |
  "$cxx" -std=c++17 -Wall -Wextra -Werror -Iinclude -x c++ -fsyntax-only -

for variant in snap snap-static; do
  echo "== $variant"
  rm -f "$out/$variant.png"
  "$out/$variant$exe" --simulated "$out/$variant.png"
  test -s "$out/$variant.png"
done
