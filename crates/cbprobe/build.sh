#!/bin/sh
set -eu

# Build the Gate 4b CBPROBE.COM utility inside ferric-dos-build:gate1.
# Pure 16-bit NASM: PCI configuration reads work from real mode with
# 32-bit port I/O, so like APMSTAT there is no 32-bit core to link -
# one flat COM is the tool.
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${ROOT_DIR}"

OUT_DIR="${ROOT_DIR}/out"
mkdir -p "${OUT_DIR}"

echo "[cbprobe] Assembling CBPROBE.COM, CBINIT.COM, WPROBE.COM, WSCAN.COM, WCONN.COM, USBPROBE.COM, USBCP.COM, EXFAT.COM, NTFS.COM, FSLOAD.COM, XREDIR.COM, USBEJECT.COM, USBDRV.SYS, AUDPROBE.COM, MIXER.COM, PWRCTL.COM, VBEPROBE.COM, VBECTL.COM, and EHCPROBE.COM..."
nasm -f bin -o "${OUT_DIR}/CBPROBE.COM" crates/cbprobe/asm/cbprobe.asm
nasm -f bin -o "${OUT_DIR}/CBINIT.COM" crates/cbprobe/asm/cbinit.asm
nasm -f bin -o "${OUT_DIR}/WPROBE.COM" crates/cbprobe/asm/wprobe.asm
nasm -f bin -o "${OUT_DIR}/WSCAN.COM" crates/cbprobe/asm/wscan.asm
nasm -f bin -o "${OUT_DIR}/WCONN.COM" crates/cbprobe/asm/wconn.asm
nasm -f bin -o "${OUT_DIR}/USBPROBE.COM" crates/cbprobe/asm/usbprobe.asm
nasm -f bin -o "${OUT_DIR}/USBCP.COM" crates/cbprobe/asm/usbcp.asm
nasm -f bin -o "${OUT_DIR}/EXFAT.COM" crates/cbprobe/asm/exfat.asm
nasm -f bin -o "${OUT_DIR}/NTFS.COM" crates/cbprobe/asm/ntfs.asm
nasm -f bin -o "${OUT_DIR}/FSLOAD.COM" crates/cbprobe/asm/fsload.asm
nasm -f bin -o "${OUT_DIR}/XREDIR.COM" crates/cbprobe/asm/xredir.asm
nasm -f bin -o "${OUT_DIR}/USBEJECT.COM" crates/cbprobe/asm/usbeject.asm
nasm -f bin -o "${OUT_DIR}/USBDRV.SYS" crates/cbprobe/asm/usbdrv.asm
nasm -f bin -o "${OUT_DIR}/AUDPROBE.COM" crates/cbprobe/asm/audprobe.asm
nasm -f bin -o "${OUT_DIR}/MIXER.COM" crates/cbprobe/asm/mixer.asm
nasm -f bin -o "${OUT_DIR}/PWRCTL.COM" crates/cbprobe/asm/pwrctl.asm
nasm -f bin -o "${OUT_DIR}/VBEPROBE.COM" crates/cbprobe/asm/vbeprobe.asm
nasm -f bin -o "${OUT_DIR}/VBECTL.COM" crates/cbprobe/asm/vbectl.asm
nasm -f bin -o "${OUT_DIR}/EHCPROBE.COM" crates/cbprobe/asm/ehcprobe.asm
nasm -f bin -o "${OUT_DIR}/SDPROBE.COM" crates/cbprobe/asm/sdprobe.asm
nasm -f bin -o "${OUT_DIR}/SF2LOAD.COM" crates/cbprobe/asm/sf2load.asm

COM_SIZE="$(wc -c < "${OUT_DIR}/CBPROBE.COM")"
INIT_SIZE="$(wc -c < "${OUT_DIR}/CBINIT.COM")"
WP_SIZE="$(wc -c < "${OUT_DIR}/WPROBE.COM")"
WS_SIZE="$(wc -c < "${OUT_DIR}/WSCAN.COM")"
WC_SIZE="$(wc -c < "${OUT_DIR}/WCONN.COM")"
UP_SIZE="$(wc -c < "${OUT_DIR}/USBPROBE.COM")"
UC_SIZE="$(wc -c < "${OUT_DIR}/USBCP.COM")"
XF_SIZE="$(wc -c < "${OUT_DIR}/EXFAT.COM")"
NT_SIZE="$(wc -c < "${OUT_DIR}/NTFS.COM")"
FL_SIZE="$(wc -c < "${OUT_DIR}/FSLOAD.COM")"
XR_SIZE="$(wc -c < "${OUT_DIR}/XREDIR.COM")"
UE_SIZE="$(wc -c < "${OUT_DIR}/USBEJECT.COM")"
UD_SIZE="$(wc -c < "${OUT_DIR}/USBDRV.SYS")"
AP_SIZE="$(wc -c < "${OUT_DIR}/AUDPROBE.COM")"
MX_SIZE="$(wc -c < "${OUT_DIR}/MIXER.COM")"
PC_SIZE="$(wc -c < "${OUT_DIR}/PWRCTL.COM")"
VP_SIZE="$(wc -c < "${OUT_DIR}/VBEPROBE.COM")"
VC_SIZE="$(wc -c < "${OUT_DIR}/VBECTL.COM")"
EH_SIZE="$(wc -c < "${OUT_DIR}/EHCPROBE.COM")"
SD_SIZE="$(wc -c < "${OUT_DIR}/SDPROBE.COM")"
SF_SIZE="$(wc -c < "${OUT_DIR}/SF2LOAD.COM")"
echo "[cbprobe] Built CBPROBE.COM (${COM_SIZE} B), CBINIT.COM (${INIT_SIZE} B), WPROBE.COM (${WP_SIZE} B), WSCAN.COM (${WS_SIZE} B), WCONN.COM (${WC_SIZE} B), USBPROBE.COM (${UP_SIZE} B), USBCP.COM (${UC_SIZE} B), EXFAT.COM (${XF_SIZE} B), NTFS.COM (${NT_SIZE} B), FSLOAD.COM (${FL_SIZE} B), XREDIR.COM (${XR_SIZE} B), USBEJECT.COM (${UE_SIZE} B), USBDRV.SYS (${UD_SIZE} B), AUDPROBE.COM (${AP_SIZE} B), MIXER.COM (${MX_SIZE} B), PWRCTL.COM (${PC_SIZE} B), VBEPROBE.COM (${VP_SIZE} B), VBECTL.COM (${VC_SIZE} B), EHCPROBE.COM (${EH_SIZE} B), SDPROBE.COM (${SD_SIZE} B), SF2LOAD.COM (${SF_SIZE} B)"
sha256sum "${OUT_DIR}/CBPROBE.COM" "${OUT_DIR}/CBINIT.COM" "${OUT_DIR}/WPROBE.COM" "${OUT_DIR}/WSCAN.COM" "${OUT_DIR}/WCONN.COM" "${OUT_DIR}/USBPROBE.COM" "${OUT_DIR}/USBCP.COM" "${OUT_DIR}/EXFAT.COM" "${OUT_DIR}/NTFS.COM" "${OUT_DIR}/FSLOAD.COM" "${OUT_DIR}/XREDIR.COM" "${OUT_DIR}/USBEJECT.COM" "${OUT_DIR}/USBDRV.SYS" "${OUT_DIR}/AUDPROBE.COM" "${OUT_DIR}/MIXER.COM" "${OUT_DIR}/PWRCTL.COM" "${OUT_DIR}/VBEPROBE.COM" "${OUT_DIR}/VBECTL.COM" "${OUT_DIR}/EHCPROBE.COM" "${OUT_DIR}/SDPROBE.COM" "${OUT_DIR}/SF2LOAD.COM"
