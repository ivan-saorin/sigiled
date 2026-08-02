#!/bin/sh
# Bootstrap a Rust dev toolchain INSIDE a SIGILED workspace container (no root,
# no docker). The v1 runtime image is debian-slim without cc: rustup alone
# cannot link host binaries (build scripts, proc macros). This script:
#   1. installs rustup (minimal, stable) if missing;
#   2. extracts gcc-14 + binutils + libc6-dev from deb.debian.org into
#      $HOME/tc via dpkg -x (no root needed) and wires a gcc wrapper as the
#      cargo linker.
# Everything lands in $HOME (ephemeral: rerun after recycle). Idempotent.
set -eu

TC=$HOME/tc
DIST=trixie
ARCH=amd64
MIRROR=https://deb.debian.org/debian

if ! command -v cargo >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    echo "== installing rustup (minimal, stable)"
    curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi
. "$HOME/.cargo/env"

if [ ! -f "$TC/debs/filelist" ]; then
    echo "== fetching toolchain debs ($DIST/$ARCH)"
    mkdir -p "$TC/debs"
    cd "$TC/debs"
    curl -s "$MIRROR/dists/$DIST/main/binary-$ARCH/Packages.gz" -o Packages.gz
    # Resolve exact pool filenames for the packages we need.
    PKGS="gcc-14 cpp-14 cpp-14-x86-64-linux-gnu libgcc-s1 libc6 gcc-14-x86-64-linux-gnu libgcc-14-dev libc6-dev binutils libbinutils binutils-x86-64-linux-gnu binutils-common libctf0 libsframe1 libjansson4 libgmp10 libmpfr6 libmpc3 libisl23 libzstd1"
    zcat Packages.gz | awk -v pkgs="$PKGS" '
        BEGIN { n = split(pkgs, a, " "); for (i = 1; i <= n; i++) want[a[i]] = 1 }
        /^Package: / { p = $2 }
        /^Filename: / { if (p in want) print $2 }
    ' > filelist
    [ "$(wc -l < filelist)" -gt 0 ] || { echo "no packages resolved"; exit 1; }
    while read -r f; do
        b=$(basename "$f")
        [ -f "$b" ] || { echo "  get $b"; curl -s "$MIRROR/$f" -O; }
        dpkg -x "$b" "$TC"
    done < filelist
fi
# Debian is merged-usr: linker scripts reference /lib/... — mirror it in the sysroot.
[ -e "$TC/lib" ] || ln -s usr/lib "$TC/lib"
[ -e "$TC/lib64" ] || ln -s usr/lib64 "$TC/lib64"

echo "== wiring gcc wrapper"
GCCDIR=$(ls -d "$TC"/usr/lib/gcc/x86_64-linux-gnu/* | head -1)
GCCBIN=$(ls "$TC"/usr/bin/*gcc-14 | head -1)
    # gcc driver needs: its own shared libs, collect2 (libexec), ld (binutils),
    # crt objects (libc6-dev + libgcc-dev) — all under $TC, never /usr.
cat > "$TC/gcc" <<W
#!/bin/sh
export LD_LIBRARY_PATH="$TC/usr/lib/x86_64-linux-gnu\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
exec "$GCCBIN" \
    --sysroot="$TC" \
    -B"$GCCDIR" \
    -B"$TC/usr/libexec/gcc/x86_64-linux-gnu/$(basename "$GCCDIR")" \
    -B"$TC/usr/bin" \
    -L"$TC/usr/lib/x86_64-linux-gnu" \
    "\$@"
W
chmod +x "$TC/gcc"
# gcc looks for plain `ld` in the -B dirs; extracted binutils may name it either way.
[ -e "$TC/usr/bin/ld" ] || ln -sf "$(cd "$TC/usr/bin" && ls *ld | head -1)" "$TC/usr/bin/ld"

# ar needs the toolchain's libbfd at runtime — same wrapper trick as gcc.
# Crates that compile C (ring, …) want CC/AR: export them per-build.
cat > "$TC/ar" <<W
#!/bin/sh
export LD_LIBRARY_PATH="$TC/usr/lib/x86_64-linux-gnu\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
exec "$TC/usr/bin/ar" "\$@"
W
chmod +x "$TC/ar"

mkdir -p "$HOME/.cargo"
if ! grep -q "tc/gcc" "$HOME/.cargo/config.toml" 2>/dev/null; then
    echo "== pointing cargo at the wrapper"
    cat >> "$HOME/.cargo/config.toml" <<W
[target.x86_64-unknown-linux-gnu]
linker = "$TC/gcc"
W
fi

echo "== smoke test"
cd /tmp && rm -rf tc-smoke && cargo new tc-smoke -q && cd tc-smoke && cargo run -q
echo "== toolchain ready"
echo "   for crates that compile C (ring, ...): export CC=$TC/gcc AR=$TC/ar"
