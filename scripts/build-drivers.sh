#!/bin/sh
# Run inside the repository's pinned Docker build container.
set -eu
cd "$(dirname "$0")/.."
for crate in ferric-res ferric-cb-provider ferric-ethernet gate1-svc ferric-exfat ferric-ntfs ferric-vbe ferric-pmu ferric-sd cbprobe; do
    sh "crates/$crate/build.sh"
done

# External VSBHDA dependencies are intentionally not part of this build.
for artifact in FRES.JLM CARDBUS.JLM I8255X.JLM ETHINIT.COM FERRIC.JLM EXFAT.JLM NTFS.JLM VBE.JLM PMU.JLM TOSHSD.JLM NETINIT.COM SDPROBE.COM PWRCTL.COM; do
    test -s "out/$artifact"
done
sha256sum out/CARDBUS.JLM out/I8255X.JLM out/ETHINIT.COM out/FRES.JLM out/FERRIC.JLM out/EXFAT.JLM out/NTFS.JLM out/VBE.JLM out/PMU.JLM out/TOSHSD.JLM
