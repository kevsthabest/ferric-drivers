#!/bin/sh
# Native host unit tests (never perform port I/O); optionally build all target modules.
# Run inside the pinned build container. Host tests assume its x86_64 host.
set -eu
cd "$(dirname "$0")/.."
case "${1:-}" in
    ""|--host-only) ;;
    *) echo "Usage: $0 [--host-only]" >&2; exit 2 ;;
esac
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
test_crate() {
    name="$1"
    path="$2"
    shift 2
    rustc --test --edition 2024 --crate-name "$name" "$path" "$@" -o "$tmp/$name-tests"
    "$tmp/$name-tests"
}
build_lib() {
    name="$1"
    path="$2"
    shift 2
    rustc --crate-type rlib --edition 2024 --crate-name "$name" "$path" "$@" -o "$tmp/lib$name.rlib"
}
for crate in abi ferric-hw wifi-core ferric-ehci ferric-i8255x ferric-cardbus; do
    name="$(printf '%s' "$crate" | tr '-' '_')"
    path="crates/$crate/src/lib.rs"
    build_lib "$name" "$path"
    test_crate "$name" "$path"
done
build_lib ferric_ath5k crates/ferric-ath5k/src/lib.rs --extern "wifi_core=$tmp/libwifi_core.rlib"
test_crate ferric_ath5k crates/ferric-ath5k/src/lib.rs --extern "wifi_core=$tmp/libwifi_core.rlib"
for crate in ferric-exfat ferric-ntfs ferric-vbe; do
    name="$(printf '%s' "$crate" | tr '-' '_')"
    test_crate "$name" "crates/$crate/src/lib.rs" --extern "abi=$tmp/libabi.rlib"
done
for crate in ferric-pmu ferric-sd ferric-res; do
    name="$(printf '%s' "$crate" | tr '-' '_')"
    test_crate "$name" "crates/$crate/src/lib.rs" --extern "ferric_hw=$tmp/libferric_hw.rlib"
done
test_crate ferric_ath_provider crates/ferric-ath-provider/src/lib.rs \
    -L "dependency=$tmp" --extern "ferric_ath5k=$tmp/libferric_ath5k.rlib" \
    --extern "ferric_cardbus=$tmp/libferric_cardbus.rlib"
test_crate ferric_cb_provider crates/ferric-cb-provider/src/lib.rs \
    --extern "ferric_hw=$tmp/libferric_hw.rlib" \
    --extern "ferric_cardbus=$tmp/libferric_cardbus.rlib"
test_crate ferric_ethernet crates/ferric-ethernet/src/lib.rs \
    --cfg 'feature="ethernet-only"' \
    --extern "ferric_hw=$tmp/libferric_hw.rlib" \
    --extern "ferric_i8255x=$tmp/libferric_i8255x.rlib"
test_crate gate1_svc crates/gate1-svc/src/lib.rs \
    -L "dependency=$tmp" --extern "ferric_ath5k=$tmp/libferric_ath5k.rlib" \
    --extern "ferric_cardbus=$tmp/libferric_cardbus.rlib" \
    --extern "abi=$tmp/libabi.rlib" \
    --extern "ferric_hw=$tmp/libferric_hw.rlib" \
    --extern "ferric_i8255x=$tmp/libferric_i8255x.rlib" \
    --extern "ferric_ehci=$tmp/libferric_ehci.rlib" \
    --extern "wifi_core=$tmp/libwifi_core.rlib"
if [ "${1:-}" != "--host-only" ]; then
    sh scripts/build-drivers.sh
fi
echo "Driver host tests passed; no emulator or silicon qualification implied."
