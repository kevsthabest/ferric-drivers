#!/bin/sh
# Report existing Linux ACPI/PNP/PCI resource inventory. No hardware writes.
# Linux itself may reconfigure hardware during boot.
set -eu
export LC_ALL=C
if [ "$(id -u)" != 0 ]; then
    echo "Run as root to avoid masked physical addresses." >&2
    exit 1
fi
read_file() {
    printf '\n--- %s ---\n' "$1"
    if [ -r "$1" ]; then
        cat "$1" || printf 'READ FAILED\n'
    else
        printf 'UNAVAILABLE\n'
    fi
}
printf 'FERRIC-LINUX-RESOURCES v1 BEGIN\n'
uname -a
for f in /proc/cmdline /sys/class/dmi/id/product_name /sys/class/dmi/id/bios_version /sys/class/dmi/id/bios_date /proc/iomem /proc/ioports; do
    read_file "$f"
done
printf '\n--- ACPI DSDT checksum ---\n'
if [ -r /sys/firmware/acpi/tables/DSDT ] && command -v sha256sum >/dev/null 2>&1; then
    sha256sum /sys/firmware/acpi/tables/DSDT || printf 'READ FAILED\n'
else
    printf 'UNAVAILABLE\n'
fi
printf '\n--- PCI sysfs resource intervals and identifiers ---\n'
for d in /sys/bus/pci/devices/*; do
    [ -d "$d" ] || continue
    for f in vendor device class resource; do read_file "$d/$f"; done
done
printf '\n--- PNP IDs and assigned resources ---\n'
for d in /sys/bus/pnp/devices/*; do
    [ -d "$d" ] || continue
    read_file "$d/id"
    read_file "$d/resources"
done
printf '\n--- ACPI namespace device IDs ---\n'
if [ -d /sys/bus/acpi/devices ]; then
    ls /sys/bus/acpi/devices
else
    printf 'UNAVAILABLE\n'
fi
printf '\n--- lspci ---\n'
if command -v lspci >/dev/null 2>&1; then
    lspci -Dnnvv || printf 'LSPCI FAILED\n'
else
    printf 'UNAVAILABLE\n'
fi
printf '\n--- kernel log ---\n'
dmesg || printf 'DMESG FAILED\n'
printf '\nFERRIC-LINUX-RESOURCES v1 END\n'
