# Ferric DOS Gate 0 pinned build environment
# Single slim base image; no package caches retained; artifacts mounted out via /work.
FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc

ARG RUST_TOOLCHAIN=nightly-2026-11-20
ARG OPENWATCOM_VERSION=2026-11-01-Build
ARG OPENWATCOM_URL=https://github.com/open-watcom/open-watcom-v2/releases/download/${OPENWATCOM_VERSION}/ow-snapshot.tar.xz
ARG OPENWATCOM_SHA256=89e90b7544cf3ea4a38124ec4c104a5027f31101405d7115c3f833daa465bb48
ARG FREECOM_VERSION=com086
ARG FREECOM_URL=https://github.com/FDOS/freecom/releases/download/${FREECOM_VERSION}/English.zip
ARG FREECOM_ZIP_SHA256=42093a00286f66bf922eebd8204a3b74b429f4c6973cd960ae1ab52ce80b4bd5
ARG FREECOM_CMD_SHA256=fc74a98b51fc10119ea2e52554915d6a405eebdb58f44b03109b952f10835c21
ARG FREECOM_XMS_CMD_SHA256=d33d3e0a3a0c02ef74d9b9e6d72a3adc7969630bacb7ec8e4e6a7cb197dcefb2
ARG JEMM_VERSION=v5.87
ARG JEMM_URL=https://github.com/Baron-von-Riedesel/Jemm/releases/download/${JEMM_VERSION}/JemmB_v587.zip
ARG JEMM_ZIP_SHA256=2584e43d02b5ebfaecd1ecd734a8b79ebef37b7df54e211190aa29e6b469f073
ARG JEMM_JEMMEX_SHA256=b9b9f7a5236973b386506dde69aef13c626deec74f0dc596ca465e5f083e7db7
ARG JEMM_JEMM386_SHA256=14ad3b0886ab1f400bb1d4e748070d54cc4ec9088f14c2a79f58cba282b2ce1e
ARG JEMM_JLOAD_SHA256=26b56ec457d21b916a3e851e7221618379a7ea41a8a7fbb6871807f7c77a94e7
ARG JEMM_QPIEMU_SHA256=005ed2b78820043cbc740e655c4821a9c9bfd8695391a7a05a8ae8799727de26
ARG JEMM_ARTISTIC_SHA256=47e9026e597eb214ec7e9c0a5fe72311b67a81d26bcf4a3cfa6ab21eb8ffeca0
ARG HDPMI_VERSION=v0.1-beta4fix2
ARG HDPMI_URL=https://github.com/crazii/HX/releases/download/${HDPMI_VERSION}/HDPMI32i.zip
ARG HDPMI_ZIP_SHA256=69a559f02a954afa2550bf8840adfaa27dad7eeeab93a12b2642c283bf3d5bcb
ARG HDPMI_EXE_SHA256=f6913b490ef30b709d94323889e2f857aa0d1bee0638e09ba6d9a917d3cb34e8

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    WATCOM=/opt/watcom \
    EDPATH=/opt/watcom/eddat \
    INCLUDE=/opt/watcom/h \
    PATH=/usr/local/cargo/bin:/opt/watcom/binl64:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    SOURCE_DATE_EPOCH=1795132800

# System toolchains, disk/FAT utilities, QEMU i386 emulator, binutils, and Python 3
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        binutils \
        ca-certificates \
        curl \
        dosfstools \
        gcc \
        libc6-dev \
        make \
        mtools \
        nasm \
        python3 \
        qemu-system-x86 \
        xz-utils && \
    rm -rf /var/lib/apt/lists/*

# Pinned dated Rust nightly matching rust-toolchain.toml, with rust-src and llvm-tools
COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN grep -q "channel = \"${RUST_TOOLCHAIN}\"" /tmp/rust-toolchain.toml && \
    rm -f /tmp/rust-toolchain.toml && \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --no-modify-path \
            --profile minimal \
            --default-toolchain "${RUST_TOOLCHAIN}" \
            --component rust-src,llvm-tools && \
    chmod -R a+rX "${RUSTUP_HOME}" "${CARGO_HOME}"

# OpenWatcom v2 pinned by dated release tag and SHA-256 content hash (not floating snapshot)
RUN curl --proto '=https' --tlsv1.2 -sSfL "${OPENWATCOM_URL}" -o /tmp/ow-snapshot.tar.xz && \
    echo "${OPENWATCOM_SHA256}  /tmp/ow-snapshot.tar.xz" | sha256sum -c - && \
    mkdir -p /opt/watcom && \
    tar -xJf /tmp/ow-snapshot.tar.xz -C /opt/watcom && \
    rm -f /tmp/ow-snapshot.tar.xz && \
    rm -rf /opt/watcom/binnt /opt/watcom/binnt64 /opt/watcom/binw /opt/watcom/binp /opt/watcom/binl /opt/watcom/binb && \
    chmod -R a+rX /opt/watcom

# FreeCOM com086 plainedt/command.com and xmsswap/command.com pinned by version and SHA-256
RUN curl --proto '=https' --tlsv1.2 -sSfL "${FREECOM_URL}" -o /tmp/freecom-english.zip && \
    echo "${FREECOM_ZIP_SHA256}  /tmp/freecom-english.zip" | sha256sum -c - && \
    mkdir -p /opt/freedos && \
    python3 -c "import zipfile; z = zipfile.ZipFile('/tmp/freecom-english.zip'); open('/opt/freedos/command.com', 'wb').write(z.read('plainedt/command.com')); open('/opt/freedos/xmscmd.com', 'wb').write(z.read('xmsswap/command.com'))" && \
    echo "${FREECOM_CMD_SHA256}  /opt/freedos/command.com" | sha256sum -c - && \
    echo "${FREECOM_XMS_CMD_SHA256}  /opt/freedos/xmscmd.com" | sha256sum -c - && \
    rm -f /tmp/freecom-english.zip && \
    chmod 0644 /opt/freedos/command.com /opt/freedos/xmscmd.com

# Jemm v5.87 runtime binaries (JEMMEX.EXE, JEMM386.EXE, JLOAD.EXE, QPIEMU.DLL, Artistic.txt) pinned by SHA-256
RUN curl --proto '=https' --tlsv1.2 -sSfL "${JEMM_URL}" -o /tmp/jemm.zip && \
    echo "${JEMM_ZIP_SHA256}  /tmp/jemm.zip" | sha256sum -c - && \
    mkdir -p /opt/freedos/jemm && \
    python3 -c "import zipfile; z = zipfile.ZipFile('/tmp/jemm.zip'); [open('/opt/freedos/jemm/' + n, 'wb').write(z.read(n)) for n in ['JEMMEX.EXE', 'JEMM386.EXE', 'JLOAD.EXE', 'QPIEMU.DLL', 'Artistic.txt']]" && \
    echo "${JEMM_JEMMEX_SHA256}  /opt/freedos/jemm/JEMMEX.EXE" | sha256sum -c - && \
    echo "${JEMM_JEMM386_SHA256}  /opt/freedos/jemm/JEMM386.EXE" | sha256sum -c - && \
    echo "${JEMM_JLOAD_SHA256}  /opt/freedos/jemm/JLOAD.EXE" | sha256sum -c - && \
    echo "${JEMM_QPIEMU_SHA256}  /opt/freedos/jemm/QPIEMU.DLL" | sha256sum -c - && \
    echo "${JEMM_ARTISTIC_SHA256}  /opt/freedos/jemm/Artistic.txt" | sha256sum -c - && \
    rm -f /tmp/jemm.zip && \
    chmod 0644 /opt/freedos/jemm/*

# HDPMI32i (crazii/HX v0.1-beta4fix2) DPMI 0.9 host pinned by SHA-256
RUN curl --proto '=https' --tlsv1.2 -sSfL "${HDPMI_URL}" -o /tmp/hdpmi32i.zip && \
    echo "${HDPMI_ZIP_SHA256}  /tmp/hdpmi32i.zip" | sha256sum -c - && \
    mkdir -p /opt/freedos/hdpmi && \
    python3 -c "import zipfile; z = zipfile.ZipFile('/tmp/hdpmi32i.zip'); open('/opt/freedos/hdpmi/hdpmi32i.exe', 'wb').write(z.read('HDPMI32i.EXE'))" && \
    echo "${HDPMI_EXE_SHA256}  /opt/freedos/hdpmi/hdpmi32i.exe" | sha256sum -c - && \
    rm -f /tmp/hdpmi32i.zip && \
    chmod 0644 /opt/freedos/hdpmi/hdpmi32i.exe

WORKDIR /work
