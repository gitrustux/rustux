#!/bin/bash
# Linker for Rust programs on Rustux. rustc links for
# x86_64-unknown-linux-musl; this passes its arguments on to ld with musl's
# start files and libraries replaced by the Rustux C library (which
# provides the part of musl's ABI that Rust's std uses; docs/RUST.md).
U=$(cd "$(dirname "$0")/.." && pwd)
args=()
for a in "$@"; do
    case "$(basename -- "$a")" in
        crt1.o|crti.o|crtbegin.o|crtend.o|crtn.o|rcrt1.o|Scrt1.o|crtbeginS.o|crtendS.o) ;;
        -lc|-lunwind|-lgcc_s|-lutil|-lrt|-lpthread|-lm|-ldl) ;;
        -pie|-static-pie|--no-dynamic-linker|--eh-frame-hdr) ;;
        *) args+=("$a") ;;
    esac
done
exec ${LD:-ld} -static -nostdlib -z noseparate-code --build-id=none -T "$U/libc/rustux.ld" \
    "$U/build/libc/crt0.o" "${args[@]}" \
    --start-group "$U/build/libc.a" "$(${CC:-gcc} -print-file-name=libgcc_eh.a)" "$(${CC:-gcc} -print-libgcc-file-name)" --end-group
