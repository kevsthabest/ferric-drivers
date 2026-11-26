//! Gate 4b Step 4b.4a — Atheros AR2413 (ath5k) MAC/EEPROM bring-up probe.
//!
//! Verified against `vendor/ref/linux-ath/ath5k/` (`pci.c`, `attach.c`,
//! `reset.c`, `reg.h`, `eeprom.h`, `ath5k.h`, pin `c44104f3`).
//! Strictly `#![no_std]`, 386-pure, and read-only with respect to storage.

use core::cell::UnsafeCell;

use crate::ath5k_initvals::{AR2413_11G_INITVALS, RF2413_11G_RFBANKS, RF2413_2GHZ_RFGAIN};
use crate::{
    bios_tick, cardbus, inb, net_activate_wifi, net_deactivate_wifi, outb,
    vdmad_lock_dma_region_pci, vdmad_unlock_dma_region,
};

pub const ATHEROS_VID: u16 = 0x168C;
pub const AR2413_DID: u16 = 0x001A;

// Verified ath5k MMIO register offsets (vendor/ref/linux-ath/ath5k/reg.h).
const AR5K_CR: usize = 0x0008;
const AR5K_CR_RXE: u32 = 0x0000_0004;
const AR5K_CR_RXD: u32 = 0x0000_0020;
const AR5K_RXDP: usize = 0x000C;
const AR5K_CFG: usize = 0x0014;
const AR5K_IER: usize = 0x0024;
const AR5K_IER_DISABLE: u32 = 0x0000_0000;
const AR5K_IER_ENABLE: u32 = 0x0000_0001;
const AR5K_TXCFG: usize = 0x0030;
const AR5K_TXCFG_SDMAMR: u32 = 0x0000_0007;
const AR5K_TXCFG_SDMAMR_128B: u32 = 0x0000_0005; // AR5K_DMASIZE_128B
const AR5K_TXCFG_TXFULL: u32 = 0x0000_03F0;
const AR5K_TXCFG_TXFULL_64B: u32 = 0x0000_0010;
const AR5K_TXCFG_DCU_DBL_BUF_DIS: u32 = 0x0000_8000;
const AR5K_RXCFG: usize = 0x0034;
const AR5K_PISR: usize = 0x0080;
const AR5K_PIMR: usize = 0x00A0;
const AR5K_RAC_PISR: usize = 0x00C0;
pub const AR5K_INT_RXOK: u32 = 0x0000_0001;
pub const AR5K_INT_TXOK: u32 = 0x0000_0040;
const AR5K_INT_NOCARD: u32 = 0xFFFF_FFFF;

// QCU 0 / DCU 0 registers (vendor/ref/linux-ath/ath5k/reg.h:567-763).
const AR5K_QUEUE_TXDP_0: usize = 0x0800; // AR5K_QUEUE_TXDP(0)
const AR5K_QCU_TXE: usize = 0x0840;
const AR5K_QCU_TXD: usize = 0x0880;
const AR5K_QCU_MISC_0: usize = 0x09C0; // AR5K_QUEUE_MISC(0)
const AR5K_DCU_QCUMASK_0: usize = 0x1000; // AR5K_QUEUE_QCUMASK(0)
const AR5K_DCU_LCL_IFS_0: usize = 0x1040; // AR5K_QUEUE_DFS_LOCAL_IFS(0)
const AR5K_DCU_RETRY_LMT_0: usize = 0x1080; // AR5K_QUEUE_DFS_RETRY_LIMIT(0)
const AR5K_DCU_MISC_0: usize = 0x1100; // AR5K_QUEUE_DFS_MISC(0)
const AR5K_DCU_MISC_FRAG_WAIT: u32 = 0x0000_0100;

const AR5K_RESET_CTL: usize = 0x4000;
const AR5K_RESET_CTL_PCU: u32 = 0x0000_0001;
const AR5K_RESET_CTL_BASEBAND: u32 = 0x0000_0002;
const AR5K_RESET_CTL_PCI: u32 = 0x0000_0010;
const AR5K_SLEEP_CTL: usize = 0x4004;
const AR5K_SLEEP_CTL_SLE: u32 = 0x0003_0000;
const AR5K_SLEEP_CTL_SLE_WAKE: u32 = 0x0000_0000;
const AR5K_INTPEND: usize = 0x4008;
const AR5K_PCICFG: usize = 0x4010;
const AR5K_PCICFG_RETRY_FIX: u32 = 0x0000_1000;
const AR5K_PCICFG_SPWR_DN: u32 = 0x0001_0000;
const AR5K_SREV: usize = 0x4020;

const AR5K_EEPROM_BASE: usize = 0x6000;
const AR5K_EEPROM_DATA: usize = 0x6004; // 5211+ / AR2413
const AR5K_EEPROM_CMD: usize = 0x6008;
const AR5K_EEPROM_CMD_READ: u32 = 0x0000_0001;
const AR5K_EEPROM_STATUS: usize = 0x600C; // 5211+ / AR2413
const AR5K_EEPROM_STAT_RDERR: u32 = 0x0000_0001;
const AR5K_EEPROM_STAT_RDDONE: u32 = 0x0000_0002;

// Station / BSSID / Key-cache registers (vendor/ref/linux-ath/ath5k/reg.h, ../reg.h).
const AR5K_STA_ID0: usize = 0x8000;
const AR5K_STA_ID1: usize = 0x8004;
const AR5K_STA_ID1_AP: u32 = 0x0001_0000;
const AR5K_STA_ID1_ADHOC: u32 = 0x0002_0000;
const AR5K_STA_ID1_PWR_SV: u32 = 0x0004_0000;
const AR5K_STA_ID1_NO_KEYSRCH: u32 = 0x0008_0000;
const AR5K_STA_ID1_DEFAULT_ANTENNA: u32 = 0x0020_0000;
const AR5K_STA_ID1_DESC_ANTENNA: u32 = 0x0040_0000;
const AR5K_STA_ID1_RTS_DEF_ANTENNA: u32 = 0x0080_0000;
const AR5K_STA_ID1_ACKCTS_6MB: u32 = 0x0100_0000;
const AR5K_STA_ID1_BASE_RATE_11B: u32 = 0x0200_0000;
const AR5K_STA_ID1_SELFGEN_DEF_ANT: u32 = 0x0400_0000;
const AR5K_STA_ID1_KEYSRCH_MODE: u32 = 0x1000_0000;
const AR5K_DEFAULT_ANTENNA: usize = 0x8058;
const AR5K_SEQ_MASK: usize = 0x8060;
const AR5K_BSS_ID0: usize = 0x8008;
const AR5K_BSS_ID1: usize = 0x800C;
const AR5K_BSS_ID1_AID_S: u32 = 16;
const AR5K_BSSMSKL: usize = 0x80E0;
const AR5K_BSSMSKU: usize = 0x80E4;
const AR5K_RX_FILTER_5211: usize = 0x803C;
const AR5K_RX_FILTER_UCAST: u32 = 0x0000_0001;
const AR5K_RX_FILTER_MCAST: u32 = 0x0000_0002;
const AR5K_RX_FILTER_BCAST: u32 = 0x0000_0004;
const AR5K_RX_FILTER_BEACON: u32 = 0x0000_0010;
const AR5K_MCAST_FILTER0_5211: usize = 0x8040;
const AR5K_MCAST_FILTER1_5211: usize = 0x8044;
const AR5K_DIAG_SW_5211: usize = 0x8048;
const AR5K_DIAG_SW_DIS_ENC: u32 = 0x0000_0008;
const AR5K_DIAG_SW_DIS_DEC: u32 = 0x0000_0010;
const AR5K_DIAG_SW_DIS_RX_5211: u32 = 0x0000_0020;

// Hardware key cache (vendor/ref/linux-ath/reg.h:42-63, key.c:143-189).
// Note: AR_KEYTABLE_TYPE_AES (5) is AES-OCB; AR_KEYTABLE_TYPE_CCM (6) is AES-CCM (WPA2 CCMP).
const AR5K_KEYTABLE_0: usize = 0x8800;
const AR5K_KEYTABLE_SIZE: usize = 128;
#[allow(dead_code)]
pub const AR5K_KEYTABLE_TYPE_AES: u32 = 0x0000_0005;
pub const AR5K_KEYTABLE_TYPE_CCM: u32 = 0x0000_0006;
const AR5K_KEYTABLE_TYPE_CLR: u32 = 0x0000_0007;
pub const AR5K_KEYTABLE_VALID: u32 = 0x0000_8000;

const AR5K_PHY_BASE: usize = 0x9800;
const AR5K_PHY_SHIFT_5GHZ: u32 = 0x0000_0007;
const AR5K_PHY_TURBO: usize = 0x9804;
const AR5K_PHY_TST1: usize = 0x9808;
const AR5K_PHY_TST1_TXHOLD: u32 = 0x0000_3800;
const AR5K_PHY_TIMING_3: usize = 0x9814;
const AR5K_PHY_TIMING_3_DSC_MAN: u32 = 0xFFFE_0000;
const AR5K_PHY_TIMING_3_DSC_MAN_S: u32 = 17;
const AR5K_PHY_TIMING_3_DSC_EXP: u32 = 0x0001_E000;
const AR5K_PHY_TIMING_3_DSC_EXP_S: u32 = 13;
const AR5K_PHY_CHIP_ID: usize = 0x9818;
const AR5K_PHY_ACT: usize = 0x981C;
const AR5K_PHY_ACT_ENABLE: u32 = 0x0000_0001;
const AR5K_PHY_RF_CTL3: usize = 0x9828;
const AR5K_PHY_RF_CTL3_TXE2XLNA_ON: u32 = 0x0000_FF00;
const AR5K_PHY_RF_CTL3_TXE2XLNA_ON_S: u32 = 8;
const AR5K_PHY_ADC_CTL: usize = 0x982C;
const AR5K_PHY_RF_CTL4: usize = 0x9834;
const AR5K_PHY_SETTLING: usize = 0x9844;
const AR5K_PHY_SETTLING_SWITCH: u32 = 0x0000_3F80;
const AR5K_PHY_SETTLING_SWITCH_S: u32 = 7;
const AR5K_PHY_GAIN: usize = 0x9848;
const AR5K_PHY_GAIN_TXRX_ATTEN: u32 = 0x0003_F000;
const AR5K_PHY_GAIN_TXRX_ATTEN_S: u32 = 12;
const AR5K_PHY_DESIRED_SIZE: usize = 0x9850;
const AR5K_PHY_DESIRED_SIZE_ADC: u32 = 0x0000_00FF;
const AR5K_PHY_DESIRED_SIZE_PGA: u32 = 0x0000_FF00;
const AR5K_PHY_DESIRED_SIZE_PGA_S: u32 = 8;
const AR5K_PHY_AGCCTL: usize = 0x9860;
const AR5K_PHY_AGCCTL_CAL: u32 = 0x0000_0001;
const AR5K_PHY_AGCCTL_NF: u32 = 0x0000_0002;
const AR5K_PHY_AGCCTL_OFDM_DIV_DIS: u32 = 0x0000_0008;
const AR5K_PHY_NF: usize = 0x9864;
const AR5K_PHY_NF_THRESH62: u32 = 0x0007_F000;
const AR5K_PHY_NF_THRESH62_S: u32 = 12;
const AR5K_PHY_PLL: usize = 0x987C;
const AR5K_PHY_PLL_RF5112_44MHZ: u32 = 0x0000_0040 | 0x0000_00AB; // RF5112 | 44MHz_5212
const AR5K_RF_BUFFER: usize = 0x989C;
const AR5K_RF_BUFFER_CONTROL_5: usize = 0x98D8;
const AR5K_PHY_ANT_CTL: usize = 0x9910;
const AR5K_PHY_ANT_CTL_TXRX_EN: u32 = 0x0000_0001;
const AR5K_PHY_ANT_CTL_SWTABLE_IDLE: u32 = 0x0000_03F0;
const AR5K_PHY_ANT_CTL_SWTABLE_IDLE_S: u32 = 4;
const AR5K_PHY_IQ: usize = 0x9920;
const AR5K_PHY_IQ_CORR_Q_Q_COFF: u32 = 0x0000_001F;
const AR5K_PHY_IQ_CORR_Q_I_COFF: u32 = 0x0000_07E0;
const AR5K_PHY_IQ_CORR_Q_I_COFF_S: u32 = 5;
const AR5K_PHY_IQ_CORR_ENABLE: u32 = 0x0000_0800;
const AR5K_PHY_TX_PWR_ADJ: usize = 0x994C;
const AR5K_PHY_TX_PWR_ADJ_CCK_GAIN_DELTA: u32 = 0x0000_0FC0;
const AR5K_PHY_TX_PWR_ADJ_CCK_GAIN_DELTA_S: u32 = 6;
const AR5K_PHY_TX_PWR_ADJ_CCK_PCDAC_INDEX: u32 = 0x00FC_0000;
const AR5K_PHY_TX_PWR_ADJ_CCK_PCDAC_INDEX_S: u32 = 18;
const AR5K_PHY_ANT_SWITCH_TABLE_0: usize = 0x9960;
const AR5K_PHY_ANT_SWITCH_TABLE_1: usize = 0x9964;
const AR5K_PHY_NFTHRES: usize = 0x9968;
const AR5K_PHY_HEAVY_CLIP_ENABLE: usize = 0x99E0;
const AR5K_PHY_ADC_TEST: usize = 0x9C24;
const AR5K_PHY_MODE: usize = 0xA200;
const AR5K_PHY_MODE_11G_2GHZ: u32 = 0x0000_0008 | 0x0000_0002 | 0x0000_0004; // RAD_RF5112 | FREQ_2GHZ | MOD_DYN
const AR5K_PHY_FAST_ANT_DIV: usize = 0xA208;
const AR5K_PHY_FAST_ANT_DIV_EN: u32 = 0x0000_2000;
const AR5K_PHY_GAIN_2GHZ: usize = 0xA20C;
const AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX: u32 = 0x00FC_0000;
const AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX_S: u32 = 18;
const AR5K_PHY_DAG_CCK_CTL: usize = 0xA228;
const AR5K_PHY_DAG_CCK_CTL_EN_RSSI_THR: u32 = 0x0000_0200;
const AR5K_PHY_DAG_CCK_CTL_RSSI_THR: u32 = 0x0001_FC00;
const AR5K_PHY_DAG_CCK_CTL_RSSI_THR_S: u32 = 10;
const AR5K_PHY_BLUETOOTH: usize = 0xA254;

// AR5212/AR2413 RX descriptor status bits (vendor/ref/linux-ath/ath5k/desc.h:78-94).
const AR5K_RX_STAT0_DATA_LEN: u32 = 0x0000_0FFF;
const AR5K_RX_STAT0_RSSI_SHIFT: u32 = 20;
const AR5K_RX_STAT1_DONE: u32 = 0x0000_0001;
const AR5K_RX_STAT1_OK: u32 = 0x0000_0002;

// AR5212/AR2413 4-word TX descriptor bits (vendor/ref/linux-ath/ath5k/desc.h:221-310).
const AR5K_4W_TX_DESC_CTL0_FRAME_LEN: u32 = 0x0000_0FFF;
const AR5K_4W_TX_DESC_CTL0_XMIT_POWER: u32 = 0x003F_0000;
const AR5K_4W_TX_DESC_CTL0_XMIT_POWER_S: u32 = 16;
const AR5K_4W_TX_DESC_CTL0_CLRDMASK: u32 = 0x0100_0000;
const AR5K_4W_TX_DESC_CTL0_ENCRYPT_KEY_VALID: u32 = 0x4000_0000;
const AR5K_4W_TX_DESC_CTL1_BUF_LEN: u32 = 0x0000_0FFF;
const AR5K_4W_TX_DESC_CTL1_ENCRYPT_KEY_IDX: u32 = 0x000F_E000;
const AR5K_4W_TX_DESC_CTL1_ENCRYPT_KEY_IDX_S: u32 = 13;
const AR5K_4W_TX_DESC_CTL1_NOACK: u32 = 0x0100_0000;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0: u32 = 0x000F_0000;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0_S: u32 = 16;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES1: u32 = 0x00F0_0000;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES1_S: u32 = 20;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES2: u32 = 0x0F00_0000;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES2_S: u32 = 24;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES3: u32 = 0xF000_0000;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES3_S: u32 = 28;
const AR5K_4W_TX_DESC_CTL3_XMIT_RATE0: u32 = 0x0000_001F;
const AR5K_4W_TX_DESC_CTL3_XMIT_RATE1_S: u32 = 5;
const AR5K_4W_TX_DESC_CTL3_XMIT_RATE2_S: u32 = 10;
const AR5K_4W_TX_DESC_CTL3_XMIT_RATE3_S: u32 = 15;
const AR5K_DESC_TX_STATUS0_FRAME_XMIT_OK: u32 = 0x0000_0001;
const AR5K_DESC_TX_STATUS1_DONE: u32 = 0x0000_0001;

// Verified AR5212/AR2413 TX hardware rate codes (vendor/ref/linux-ath/ath5k/ath5k.h:898-910).
pub const ATH5K_RATE_CODE_1M: u8 = 0x1B;
pub const ATH5K_RATE_CODE_6M: u8 = 0x0B;
pub const ATH5K_RATE_CODE_12M: u8 = 0x0A;
pub const ATH5K_RATE_CODE_18M: u8 = 0x0E;
pub const ATH5K_RATE_CODE_24M: u8 = 0x09;
pub const ATH5K_RATE_CODE_36M: u8 = 0x0D;
pub const ATH5K_RATE_CODE_54M: u8 = 0x0C;

// Verified SREV constants (vendor/ref/linux-ath/ath5k/ath5k.h:320-326).
pub const AR5K_SREV_AR5212_V4: u32 = 0x54;
pub const AR5K_SREV_AR5213A: u32 = 0x59;
pub const AR5K_SREV_AR2413: u32 = 0x78;

// Verified EEPROM word offsets (vendor/ref/linux-ath/ath5k/eeprom.h).
const AR5K_EEPROM_MAGIC: u32 = 0x003D;
pub const AR5K_EEPROM_MAGIC_VALUE: u16 = 0x5AA5;
const AR5K_EEPROM_REG_DOMAIN: u32 = 0x00BF;
const AR5K_EEPROM_VERSION: u32 = 0x00C1;
pub const AR5K_EEPROM_VERSION_5_0: u16 = 0x5000;
const AR5K_EEPROM_HDR: u32 = 0x00C2;
const AR5K_EEPROM_MISC5: u32 = 0x00C9;
const AR5K_EEPROM_AES_DIS_BIT: u16 = 0x0002;

/// Determine hardware AES-CCM support per Linux `ath5k/attach.c:315-318`.
pub const fn decode_aes_support(srev: u32, ee_version: u16, ee_misc5: u16) -> bool {
    let mac_srev = srev & 0xFF;
    mac_srev >= AR5K_SREV_AR5212_V4
        && (ee_version < AR5K_EEPROM_VERSION_5_0 || (ee_misc5 & AR5K_EEPROM_AES_DIS_BIT) == 0)
}

/// Decode the 6-byte station MAC from EEPROM words `0x1F`, `0x1E`, `0x1D`
/// per Linux `ath5k/pci.c:129-141`. Rejects all-zero and all-FF reads.
pub const fn decode_eeprom_mac(w1f: u16, w1e: u16, w1d: u16) -> Option<[u8; 6]> {
    let total = (w1f as u32) + (w1e as u32) + (w1d as u32);
    if total == 0 || total == 3 * 0xFFFF {
        return None;
    }
    Some([
        (w1f >> 8) as u8,
        (w1f & 0xFF) as u8,
        (w1e >> 8) as u8,
        (w1e & 0xFF) as u8,
        (w1d >> 8) as u8,
        (w1d & 0xFF) as u8,
    ])
}

/// Fixed 44-byte `#[repr(C)]` AR2413 probe summary returned to `WIFIPROBE.COM`
/// via JLM V86 API `AX=0x000A`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct WifiProbeInfo {
    pub present: u8,
    pub stage: u8,
    pub slot: u8,
    pub aes_hw: u8,
    pub pci_vid: u16,
    pub pci_did: u16,
    pub bar0_phys: u32,
    pub bar0_lin: u32,
    pub srev: u32,
    pub phy_id: u32,
    pub ee_magic: u16,
    pub ee_version: u16,
    pub ee_hdr: u16,
    pub ee_misc5: u16,
    pub ee_regdom: u16,
    pub mac_ok: u8,
    pub reserved: u8,
    pub mac: [u8; 6],
    pub pad: [u8; 2],
}

impl WifiProbeInfo {
    pub const ZERO: Self = Self {
        present: 0,
        stage: 0,
        slot: 0,
        aes_hw: 0,
        pci_vid: 0,
        pci_did: 0,
        bar0_phys: 0,
        bar0_lin: 0,
        srev: 0,
        phy_id: 0,
        ee_magic: 0,
        ee_version: 0,
        ee_hdr: 0,
        ee_misc5: 0,
        ee_regdom: 0,
        mac_ok: 0,
        reserved: 0,
        mac: [0; 6],
        pad: [0; 2],
    };
}

const _: () = {
    assert!(core::mem::size_of::<WifiProbeInfo>() == 44);
};

struct Ath5kState {
    mapped_phys: u32,
    mapped_lin: u32,
    last_probe: WifiProbeInfo,
    connected: bool,
    dma_lin: u32,
    dma_phys: u32,
    rx_idx: usize,
    sta_mac: [u8; 6],
    bssid: [u8; 6],
    channel: u8,
    protected: bool,
    ptk_tk: [u8; 16],
    // The completed WPA2 handshake machine, kept for the life of the
    // session so post-connect group-key rotations can be authenticated and
    // unwrapped. None on open networks and after disconnect; a reconnect
    // always builds a fresh machine with a fresh SNonce.
    hs: Option<wifi_core::handshake::Wpa2Handshake>,
    tx_seq: u16,
    tx_pn: u64,
    // Live diagnostic telemetry (read via WPROBE /D -> JLM AX=0x000D).
    tx_total: u32,
    tx_ok: u32,
    tx_fail_retry: u32,
    tx_fail_fifo: u32,
    tx_fail_filt: u32,
    tx_timeout: u32,
    tx_retries_sum: u32,
    tx_last_stat0: u32,
    tx_last_stat1: u32,
    rx_total: u32,
    rx_ok: u32,
    rx_err_key: u32,
    rx_err_dec: u32,
    rx_err_mic: u32,
    rx_err_crc: u32,
    rx_err_phy: u32,
    rx_err_other: u32,
    rx_decap_ok: u32,
    rx_decap_fail: u32,
    rx_ucast: u32,
    rx_ucast_arp: u32,
    rx_ucast_ip: u32,
    rx_nondata: u32,
    rx_nondata_last_fc: u16,
    rx_rearm: u32,
    rx_last_rssi: u8,
    rx_last_stat0: u32,
    rx_last_stat1: u32,
    wifi_ticks: u32,
    last_tx_len: u16,
    last_tx_eth: [u8; 48],
    last_rxu_len: u16,
    last_rxu_rate: u8,
    last_fail: u8,
    self_healed: u8,
    srev_first: u32,
    pci_pri_before: u16,
    pci_sec_before: u16,
    pci_pri_after: u16,
    pci_sec_after: u16,
    last_rxu_eth: [u8; 48],
    // Static Ring-0 scratch buffers (Jemm's Ring-0 stack is small ~2 KiB;
    // never place large frame buffers on the Ring-0 stack!).
    tx_frame: [u8; 256],
    eth_rx: [u8; 512],
    eapol_reply: [u8; 256],
    eth_tx: [u8; 280],
    dot11_tx: [u8; 1600],
    scan_res: WifiScanResult,
}

impl Ath5kState {
    const fn new() -> Self {
        Self {
            mapped_phys: 0,
            mapped_lin: 0,
            last_probe: WifiProbeInfo::ZERO,
            connected: false,
            dma_lin: 0,
            dma_phys: 0,
            rx_idx: 0,
            sta_mac: [0; 6],
            bssid: [0; 6],
            channel: 0,
            protected: false,
            ptk_tk: [0; 16],
            hs: None,
            tx_seq: 0,
            tx_pn: 1,
            tx_total: 0,
            tx_ok: 0,
            tx_fail_retry: 0,
            tx_fail_fifo: 0,
            tx_fail_filt: 0,
            tx_timeout: 0,
            tx_retries_sum: 0,
            tx_last_stat0: 0,
            tx_last_stat1: 0,
            rx_total: 0,
            rx_ok: 0,
            rx_err_key: 0,
            rx_err_dec: 0,
            rx_err_mic: 0,
            rx_err_crc: 0,
            rx_err_phy: 0,
            rx_err_other: 0,
            rx_decap_ok: 0,
            rx_decap_fail: 0,
            rx_ucast: 0,
            rx_ucast_arp: 0,
            rx_ucast_ip: 0,
            rx_nondata: 0,
            rx_nondata_last_fc: 0,
            rx_rearm: 0,
            rx_last_rssi: 0,
            rx_last_stat0: 0,
            rx_last_stat1: 0,
            wifi_ticks: 0,
            last_tx_len: 0,
            last_tx_eth: [0; 48],
            last_rxu_len: 0,
            last_rxu_rate: 0,
            last_fail: 0,
            self_healed: 0,
            srev_first: 0,
            pci_pri_before: 0,
            pci_sec_before: 0,
            pci_pri_after: 0,
            pci_sec_after: 0,
            last_rxu_eth: [0; 48],
            tx_frame: [0; 256],
            eth_rx: [0; 512],
            eapol_reply: [0; 256],
            eth_tx: [0; 280],
            dot11_tx: [0; 1600],
            scan_res: WifiScanResult::ZERO,
        }
    }
}

struct SingleCoreCellAth(UnsafeCell<Ath5kState>);
// SAFETY: single-core DOS with CLI/STI discipline.
unsafe impl Sync for SingleCoreCellAth {}

static ATH5K_STATE: SingleCoreCellAth = SingleCoreCellAth(UnsafeCell::new(Ath5kState::new()));

fn ath_state() -> &'static mut Ath5kState {
    unsafe { &mut *ATH5K_STATE.0.get() }
}

pub fn ath5k_reset() {
    *ath_state() = Ath5kState::new();
}

unsafe fn reg_r32(base: u32, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + off) as *const u32) }
}

unsafe fn reg_w32(base: u32, off: usize, val: u32) {
    unsafe { core::ptr::write_volatile((base as usize + off) as *mut u32, val) }
}

unsafe fn wake_chip(base: u32) -> bool {
    unsafe {
        let mut staid = reg_r32(base, AR5K_STA_ID1);
        staid &= !AR5K_STA_ID1_PWR_SV;

        let mut data = reg_r32(base, AR5K_SLEEP_CTL);
        if (data & 0xFFC0_0000) != 0 {
            data = 0;
        } else {
            data &= !AR5K_SLEEP_CTL_SLE;
        }
        reg_w32(base, AR5K_SLEEP_CTL, data | AR5K_SLEEP_CTL_SLE_WAKE);
        cardbus::delay_ms(1);

        let mut awake = false;
        for _ in 0..100 {
            if (reg_r32(base, AR5K_PCICFG) & AR5K_PCICFG_SPWR_DN) == 0 {
                awake = true;
                break;
            }
            cardbus::delay_ms(1);
            reg_w32(base, AR5K_SLEEP_CTL, data | AR5K_SLEEP_CTL_SLE_WAKE);
        }
        if !awake {
            return false;
        }
        reg_w32(base, AR5K_STA_ID1, staid);
        true
    }
}

unsafe fn nic_reset(base: u32, val: u32) -> bool {
    unsafe {
        let _ = reg_r32(base, AR5K_RXDP);
        reg_w32(base, AR5K_RESET_CTL, val);
        cardbus::delay_ms(1);

        let mask = AR5K_RESET_CTL_PCU | AR5K_RESET_CTL_BASEBAND;
        let expected = val & mask;
        let mut ok = false;
        for _ in 0..100 {
            if (reg_r32(base, AR5K_RESET_CTL) & mask) == expected {
                ok = true;
                break;
            }
            cardbus::delay_ms(1);
        }
        if (val & AR5K_RESET_CTL_PCU) == 0 {
            reg_w32(base, AR5K_CFG, 0);
        }
        ok
    }
}

unsafe fn eeprom_read(base: u32, offset: u32) -> Option<u16> {
    unsafe {
        reg_w32(base, AR5K_EEPROM_BASE, offset);
        let cmd = reg_r32(base, AR5K_EEPROM_CMD);
        reg_w32(base, AR5K_EEPROM_CMD, cmd | AR5K_EEPROM_CMD_READ);

        for _ in 0..5000 {
            let status = reg_r32(base, AR5K_EEPROM_STATUS);
            if (status & AR5K_EEPROM_STAT_RDDONE) != 0 {
                if (status & AR5K_EEPROM_STAT_RDERR) != 0 {
                    return None;
                }
                return Some((reg_r32(base, AR5K_EEPROM_DATA) & 0xFFFF) as u16);
            }
        }
        None
    }
}

/// One bounded self-heal attempt for a card whose SREV reads back all-ones
/// or zero right after stage-5 power-up — the signature of a late internal
/// reset clearing the BAR0/CMD programming the bridge just wrote. Re-asserts
/// the stage-5 configuration, wakes the chip, and re-reads SREV exactly
/// once. Returns the SREV to continue with: the re-read value if a heal was
/// attempted, otherwise the value passed in.
unsafe fn srev_heal_once(slot: u8, bar0_lin: u32, srev: u32) -> u32 {
    if srev != 0xFFFF_FFFF && srev != 0 {
        return srev;
    }
    unsafe { cardbus::reassert_stage5(slot) };
    let _ = unsafe { wake_chip(bar0_lin) };
    cardbus::delay_ms(10);
    let healed = unsafe { reg_r32(bar0_lin, AR5K_SREV) };
    if healed != 0xFFFF_FFFF && healed != 0 {
        ath_state().self_healed = 1;
    }
    healed
}

fn probe_internal() -> WifiProbeInfo {
    let mut info = WifiProbeInfo::ZERO;
    let Some((slot, did, bar0_phys)) = cardbus::find_card_bar0(ATHEROS_VID) else {
        return info;
    };

    info.present = 1;
    info.stage = 1;
    info.slot = slot;
    info.pci_vid = ATHEROS_VID;
    info.pci_did = did;
    info.bar0_phys = bar0_phys;

    // Map 64 KiB (16 pages) of BAR0 uncached, reusing any existing mapping for the same phys base.
    let st = ath_state();
    let bar0_lin = if st.mapped_phys == bar0_phys && st.mapped_lin != 0 {
        st.mapped_lin
    } else {
        let lin = unsafe { cardbus::map_mmio(bar0_phys, 16) };
        if lin != 0 {
            st.mapped_phys = bar0_phys;
            st.mapped_lin = lin;
        }
        lin
    };
    if bar0_lin == 0 {
        return info;
    }
    info.bar0_lin = bar0_lin;
    info.stage = 2;

    // Capture bridge primary/secondary PCI status immediately before & after the
    // very first AR5K_SREV read (before any wake/self-heal alters evidence).
    let (pri0, sec0) = unsafe { cardbus::read_bridge_pci_status(slot) };
    let srev = unsafe { reg_r32(bar0_lin, AR5K_SREV) };
    let (pri1, sec1) = unsafe { cardbus::read_bridge_pci_status(slot) };
    st.srev_first = srev;
    st.pci_pri_before = pri0;
    st.pci_sec_before = sec0;
    st.pci_pri_after = pri1;
    st.pci_sec_after = sec1;
    st.self_healed = 0;

    let srev = unsafe { srev_heal_once(slot, bar0_lin, srev) };
    info.srev = srev;
    if srev == 0xFFFF_FFFF || srev == 0 {
        return info;
    }

    if !unsafe { wake_chip(bar0_lin) } {
        return info;
    }
    if !unsafe {
        nic_reset(
            bar0_lin,
            AR5K_RESET_CTL_PCU | AR5K_RESET_CTL_BASEBAND | AR5K_RESET_CTL_PCI,
        )
    } {
        return info;
    }
    if !unsafe { wake_chip(bar0_lin) } {
        return info;
    }
    if !unsafe { nic_reset(bar0_lin, 0) } {
        return info;
    }
    if (srev & 0xFF) >= AR5K_SREV_AR5213A {
        unsafe {
            let pcfg = reg_r32(bar0_lin, AR5K_PCICFG);
            reg_w32(bar0_lin, AR5K_PCICFG, pcfg | AR5K_PCICFG_RETRY_FIX);
        }
    }
    info.phy_id = unsafe { reg_r32(bar0_lin, AR5K_PHY_CHIP_ID) };
    info.stage = 3;

    // Read EEPROM magic, version, header, misc5 (AES-disable), regdomain, and MAC.
    let Some(magic) = (unsafe { eeprom_read(bar0_lin, AR5K_EEPROM_MAGIC) }) else {
        return info;
    };
    let Some(ver) = (unsafe { eeprom_read(bar0_lin, AR5K_EEPROM_VERSION) }) else {
        return info;
    };
    let Some(hdr) = (unsafe { eeprom_read(bar0_lin, AR5K_EEPROM_HDR) }) else {
        return info;
    };
    let Some(misc5) = (unsafe { eeprom_read(bar0_lin, AR5K_EEPROM_MISC5) }) else {
        return info;
    };
    let Some(regdom) = (unsafe { eeprom_read(bar0_lin, AR5K_EEPROM_REG_DOMAIN) }) else {
        return info;
    };
    info.ee_magic = magic;
    info.ee_version = ver;
    info.ee_hdr = hdr;
    info.ee_misc5 = misc5;
    info.ee_regdom = regdom;
    info.aes_hw = u8::from(decode_aes_support(srev, ver, misc5));

    // Read MAC per ath5k_pci_eeprom_read_mac (prime 0x20, then 0x1F, 0x1E, 0x1D).
    let _ = unsafe { eeprom_read(bar0_lin, 0x20) };
    if let (Some(w1f), Some(w1e), Some(w1d)) = (
        unsafe { eeprom_read(bar0_lin, 0x1F) },
        unsafe { eeprom_read(bar0_lin, 0x1E) },
        unsafe { eeprom_read(bar0_lin, 0x1D) },
    ) {
        if let Some(mac) = decode_eeprom_mac(w1f, w1e, w1d) {
            info.mac = mac;
            info.mac_ok = 1;
        }
    }

    info.stage = 4;
    info
}

/// FFI entry for JLM V86 API `AX=0x000A`: probe Atheros AR2413 CardBus NIC,
/// wake/reset MAC, read EEPROM & hardware AES-CCM capability, and copy the
/// 44-byte `WifiProbeInfo` into `dst_lin`. Returns 1 if `stage == 4`, else 0.
///
/// While a session is connected this refuses without touching the hardware
/// (zeroed info, return 0): probing wakes and resets the chip, which would
/// silently destroy the live session's keys, channel, and BSSID programming
/// while the packet facade kept routing to Wi-Fi.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_wifi_probe(dst_lin: u32) -> u32 {
    if ath_state().connected {
        if dst_lin != 0 {
            let info = WifiProbeInfo::ZERO;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    &info as *const WifiProbeInfo as *const u8,
                    dst_lin as *mut u8,
                    core::mem::size_of::<WifiProbeInfo>(),
                );
            }
        }
        return 0;
    }
    let info = probe_internal();
    ath_state().last_probe = info;
    if dst_lin != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                &info as *const WifiProbeInfo as *const u8,
                dst_lin as *mut u8,
                core::mem::size_of::<WifiProbeInfo>(),
            );
        }
    }
    u32::from(info.stage >= 4)
}

// ---------------------------------------------------------------------
// Gate 4b Step 4b.4b — AR2413 RF2413 2.4 GHz initvals, channel tuning,
// RX DMA descriptor ring, and passive beacon scan via `wifi-core`.
// ---------------------------------------------------------------------

const WIFI_DMA_SIZE: usize = 16384; // 16 KiB (1024 DOS paragraphs)
const RX_DESC_COUNT: usize = 8;
const RX_DESC_STRIDE: usize = 64;
const RX_BUF_BASE: usize = 0x0400; // 1 KiB-aligned after RX (0x000..0x200) + TX (0x300) descs
const RX_BUF_SIZE: usize = 1600; // 8 * 1600 = 0x3200 -> 0x0400..0x3600 (before TX_BUF 0x3800)

const fn bitswap8(mut v: u32) -> u32 {
    let mut out = 0u32;
    let mut i = 0;
    while i < 8 {
        out = (out << 1) | (v & 1);
        v >>= 1;
        i += 1;
    }
    out
}

/// Compute `(rf_buf_lo, rf_buf_hi)` for RF5112/RF2413 2.4 GHz channels 1..=13
/// (`freq = 2407 + 5 * ch` MHz) per Linux `ath5k/phy.c:1335-1373`.
pub const fn rf5112_channel_regs(freq_mhz: u16) -> Option<(u32, u32)> {
    if freq_mhz < 2412 || freq_mhz > 2472 || ((freq_mhz - 2192) % 5) != 0 {
        return None;
    }
    let c = freq_mhz as u32;
    let raw = ((2 * (c - 672)) - 3040) / 10;
    let data0 = bitswap8((raw << 2) & 0xFF);
    let data = (data0 << 4) | 0x1001;
    Some((data & 0xFF, (data >> 8) & 0x7F))
}

/// Decoded AR5212/AR2413 RX descriptor status words (`desc.h:78-94`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct RxDescDecode {
    pub done: bool,
    pub ok: bool,
    pub len: u16,
    pub rssi: u8,
}

pub const fn decode_5212_rx_status(stat0: u32, stat1: u32) -> RxDescDecode {
    RxDescDecode {
        done: (stat1 & AR5K_RX_STAT1_DONE) != 0,
        ok: (stat1 & AR5K_RX_STAT1_OK) != 0,
        len: (stat0 & AR5K_RX_STAT0_DATA_LEN) as u16,
        rssi: ((stat0 >> AR5K_RX_STAT0_RSSI_SHIFT) & 0xFF) as u8,
    }
}

/// Fixed 44-byte `#[repr(C)]` AP scan entry returned inside `WifiScanResult`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ScanEntry {
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: u8,
    pub has_rsn: u8,
    pub has_privacy: u8,
    pub ssid_len: u8,
    pub pad: u8,
    pub ssid: [u8; 32],
}

impl ScanEntry {
    pub const ZERO: Self = Self {
        bssid: [0; 6],
        channel: 0,
        rssi: 0,
        has_rsn: 0,
        has_privacy: 0,
        ssid_len: 0,
        pad: 0,
        ssid: [0; 32],
    };
}

/// Fixed 720-byte `#[repr(C)]` scan summary returned to `WSCAN.COM` via
/// JLM V86 API `AX=0x000B`.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct WifiScanResult {
    pub present: u8,
    pub stage: u8,
    pub ap_count: u8,
    pub reserved: u8,
    pub rx_total: u32,
    pub rx_ok: u32,
    pub beacons: u32,
    pub entries: [ScanEntry; 16],
}

impl WifiScanResult {
    pub const ZERO: Self = Self {
        present: 0,
        stage: 0,
        ap_count: 0,
        reserved: 0,
        rx_total: 0,
        rx_ok: 0,
        beacons: 0,
        entries: [ScanEntry::ZERO; 16],
    };

    fn record_beacon(&mut self, b: &wifi_core::frame::BeaconInfo<'_>, ch: u8, rssi: u8) {
        let count = self.ap_count as usize;
        for i in 0..count {
            if self.entries[i].bssid == b.bssid {
                if rssi > self.entries[i].rssi {
                    self.entries[i].rssi = rssi;
                }
                if self.entries[i].ssid_len == 0 && !b.ssid.is_empty() {
                    let n = b.ssid.len().min(32);
                    self.entries[i].ssid_len = n as u8;
                    self.entries[i].ssid[..n].copy_from_slice(&b.ssid[..n]);
                }
                return;
            }
        }
        if count >= self.entries.len() {
            return;
        }
        let mut e = ScanEntry::ZERO;
        e.bssid = b.bssid;
        e.channel = b.channel.unwrap_or(ch);
        e.rssi = rssi;
        e.has_rsn = u8::from(b.rsn.is_some());
        e.has_privacy = u8::from((b.capability & 0x0010) != 0);
        let n = b.ssid.len().min(32);
        e.ssid_len = n as u8;
        e.ssid[..n].copy_from_slice(&b.ssid[..n]);
        self.entries[count] = e;
        self.ap_count = (count + 1) as u8;
    }
}

const _: () = {
    assert!(core::mem::size_of::<ScanEntry>() == 44);
    assert!(core::mem::size_of::<WifiScanResult>() == 720);
};

unsafe fn dma_w32(base: u32, off: usize, val: u32) {
    unsafe { core::ptr::write_volatile((base as usize + off) as *mut u32, val) }
}

unsafe fn dma_r32(base: u32, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + off) as *const u32) }
}

unsafe fn write_reg_table(base: u32, table: &[(u16, u32)]) {
    for (i, &(reg, val)) in table.iter().enumerate() {
        if (i & 7) == 7 {
            let _ = unsafe { reg_r32(base, AR5K_SREV) };
        }
        unsafe { reg_w32(base, reg as usize, val) };
    }
}

unsafe fn stop_rx_dma(base: u32) {
    unsafe {
        reg_w32(base, AR5K_CR, AR5K_CR_RXD);
        for _ in 0..50 {
            if (reg_r32(base, AR5K_CR) & AR5K_CR_RXE) == 0 {
                break;
            }
            cardbus::delay_ms(1);
        }
    }
}

unsafe fn init_rx_ring(dma_lin: u32, dma_phys: u32) {
    for i in 0..RX_DESC_COUNT {
        let doff = i * RX_DESC_STRIDE;
        let next_phys = dma_phys + (((i + 1) % RX_DESC_COUNT) * RX_DESC_STRIDE) as u32;
        let buf_phys = dma_phys + (RX_BUF_BASE + i * RX_BUF_SIZE) as u32;
        unsafe {
            dma_w32(dma_lin, doff + 0, next_phys); // ds_link
            dma_w32(dma_lin, doff + 4, buf_phys); // ds_data
            dma_w32(dma_lin, doff + 8, 0); // rx_control_0
            dma_w32(dma_lin, doff + 12, RX_BUF_SIZE as u32); // rx_control_1
            dma_w32(dma_lin, doff + 16, 0); // rx_status_0
            dma_w32(dma_lin, doff + 20, 0); // rx_status_1
        }
    }
}

/// Decoded 11g antenna switch tables & RF settling/attenuation from EEPROM
/// words `AR5K_EEPROM_MODES_11G(ee_ver) + 0..5` (`vendor/ref/linux-ath/ath5k/eeprom.c:189-243`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct EepromAnts {
    pub switch_settling: u32,
    pub atn_tx_rx: u32,
    pub ant_idle: u32,
    pub swtable_a: u32,
    pub swtable_b: u32,
}

pub const fn decode_eeprom_ants(w: [u16; 5]) -> EepromAnts {
    let switch_settling = ((w[0] >> 8) & 0x7F) as u32;
    let atn_tx_rx = ((w[0] >> 2) & 0x3F) as u32;

    let a0 = (((w[0] << 4) & 0x3F) | ((w[1] >> 12) & 0x0F)) as u32;
    let a1 = ((w[1] >> 6) & 0x3F) as u32;
    let a2 = (w[1] & 0x3F) as u32;
    let a3 = ((w[2] >> 10) & 0x3F) as u32;
    let a4 = ((w[2] >> 4) & 0x3F) as u32;
    let a5 = (((w[2] << 2) & 0x3F) | ((w[3] >> 14) & 0x03)) as u32;
    let a6 = ((w[3] >> 8) & 0x3F) as u32;
    let a7 = ((w[3] >> 2) & 0x3F) as u32;
    let a8 = (((w[3] << 4) & 0x3F) | ((w[4] >> 12) & 0x0F)) as u32;
    let a9 = ((w[4] >> 6) & 0x3F) as u32;
    let a10 = (w[4] & 0x3F) as u32;

    let swtable_a = a1 | (a2 << 6) | (a3 << 12) | (a4 << 18) | (a5 << 24);
    let swtable_b = a6 | (a7 << 6) | (a8 << 12) | (a9 << 18) | (a10 << 24);

    EepromAnts {
        switch_settling,
        atn_tx_rx,
        ant_idle: a0,
        swtable_a,
        swtable_b,
    }
}

/// Decoded 11g board-specific PHY calibration fields from EEPROM words
/// immediately following the 5 antenna words (`vendor/ref/linux-ath/ath5k/eeprom.c:249-418`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct EepromModes11g {
    pub adc_desired_size: u8,
    pub pga_desired_size: u8,
    pub tx_end2xlna_enable: u8,
    pub thr_62: u8,
    pub tx_end2xpa_disable: u8,
    pub tx_frm2xpa_enable: u8,
    pub noise_floor_thr: i8,
    pub margin_tx_rx: u8,
    pub i_cal: u8,
    pub q_cal: u8,
    pub cck_ofdm_power_delta: i8,
    pub cck_ofdm_gain_delta: i8,
    pub ob_1: u8,
    pub db_1: u8,
}

/// Pure decoder for the up to 12 EEPROM 11g mode words starting at
/// `AR5K_EEPROM_MODES_11G(ee_ver) + 5` (`eeprom.c:249-418`).
pub const fn decode_eeprom_modes_11g(ee_ver: u16, m: &[u16; 12]) -> EepromModes11g {
    let adc_desired_size = ((m[0] >> 8) & 0xFF) as u8;
    let ob_1 = ((m[0] >> 4) & 0x07) as u8;
    let db_1 = (m[0] & 0x07) as u8;

    let tx_end2xlna_enable = ((m[1] >> 8) & 0xFF) as u8;
    let thr_62 = if ee_ver <= 0x3002 {
        28u8
    } else {
        (m[1] & 0xFF) as u8
    };

    let tx_end2xpa_disable = ((m[2] >> 8) & 0xFF) as u8;
    let tx_frm2xpa_enable = (m[2] & 0xFF) as u8;

    let pga_desired_size = ((m[3] >> 8) & 0xFF) as u8;
    let noise_floor_thr = if ee_ver <= 0x3002 {
        -1i8
    } else {
        (m[3] & 0xFF) as u8 as i8
    };

    // m[4] holds xlna_gain / x_gain / xpd / fixed_bias (used by RF5111/5112).
    // m[5] is read when ee_ver >= 0x3003 (false_detect, ob_0, db_0, i_gain_lo).
    // m[6] is read when ee_ver >= 0x3004 (i_gain_hi, cck_ofdm_power_delta, scaled_cck_delta).
    let mut idx = 5usize;
    if ee_ver >= 0x3003 {
        idx += 1;
    }
    let cck_ofdm_power_delta = if ee_ver >= 0x3004 {
        let val = m[idx];
        idx += 1;
        ((val >> 3) & 0xFF) as u8 as i8
    } else {
        15i8 // AR5K_EEPROM_CCK_OFDM_DELTA
    };

    let mut margin_tx_rx = 0u8;
    let mut i_cal = 0u8;
    let mut q_cal = 0u8;
    let mut cck_ofdm_gain_delta = 0i8;

    if ee_ver >= 0x4000 {
        // Skip pwr_cal_g[0..1] (m[idx]) and turbo_max_power/xr_power (m[idx+1]).
        idx += 2;
        // m[idx]: pwr_cal_g[2] + margin_tx_rx (ee_ver >= 0x4001).
        if ee_ver >= 0x4001 {
            margin_tx_rx = ((m[idx] >> 8) & 0x3F) as u8;
        }
        idx += 1;
        // m[idx]: i_cal / q_cal.
        i_cal = ((m[idx] >> 5) & 0x3F) as u8;
        q_cal = (m[idx] & 0x1F) as u8;
        idx += 1;
        if ee_ver >= 0x4002 {
            cck_ofdm_gain_delta = (m[idx] & 0xFF) as u8 as i8;
        }
    }

    EepromModes11g {
        adc_desired_size,
        pga_desired_size,
        tx_end2xlna_enable,
        thr_62,
        tx_end2xpa_disable,
        tx_frm2xpa_enable,
        noise_floor_thr,
        margin_tx_rx,
        i_cal,
        q_cal,
        cck_ofdm_power_delta,
        cck_ofdm_gain_delta,
        ob_1,
        db_1,
    }
}

/// Verify the AR2413 EEPROM XOR checksum (`0x00C0..eep_end == 0xFFFF`) per
/// `vendor/ref/linux-ath/ath5k/eeprom.c:96-126`.
unsafe fn ar2413_verify_eeprom_checksum(base: u32, ee_ver: u16) -> bool {
    if ee_ver < 0x3000 {
        return true;
    }
    const INFO_BASE: u32 = 0x00C0;
    const INFO_MAX: u32 = 0x0400 - INFO_BASE;
    let mut eep_max = INFO_MAX;
    let Some(upper) = (unsafe { eeprom_read(base, 0x001C) }) else {
        return false;
    };
    if upper != 0 {
        let Some(lower) = (unsafe { eeprom_read(base, 0x001B) }) else {
            return false;
        };
        let endloc = (((upper as u32) & 0xFFF0) << 12) | (lower as u32);
        if endloc <= INFO_BASE {
            return false;
        }
        eep_max = endloc - INFO_BASE;
        if eep_max > 3 * INFO_MAX {
            return false;
        }
    }
    let mut cksum = 0u16;
    for off in 0..eep_max {
        let Some(val) = (unsafe { eeprom_read(base, INFO_BASE + off) }) else {
            return false;
        };
        cksum ^= val;
    }
    cksum == 0xFFFF
}

/// Patch RF2413 Bank-6 2.4 GHz Output Bias (`OB`) and Driver Bias (`DB`) into
/// `RF2413_11G_RFBANKS[23..=24]` per `vendor/ref/linux-ath/ath5k/rfbuffer.h:524-525`
/// and `phy.c:238-265,929-940`.
pub const fn patch_rf2413_ob_db(val23: u32, val24: u32, ob: u8, db: u8) -> (u32, u32) {
    let ob_swap = (((ob & 1) << 2) | (ob & 2) | ((ob >> 2) & 1)) as u32;
    let db_swap = (((db & 1) << 2) | (db & 2) | ((db >> 2) & 1)) as u32;
    // DB_2GHZ: len=3, pos=165 (entry 23, bits 4..=6).
    // OB_2GHZ: len=3, pos=168 (entry 23 bit 7 + entry 24 bits 0..=1).
    let out23 = (val23 & !0x0000_00F0) | ((db_swap << 4) & 0x0000_0070) | ((ob_swap & 1) << 7);
    let out24 = (val24 & !0x0000_0003) | ((ob_swap >> 1) & 0x0000_0003);
    (out23, out24)
}

/// Compute AR5212/AR2413 OFDM delta-slope timing mantissa & exponent for
/// `freq_mhz` (normal 20 MHz, 40 MHz clock) per `vendor/ref/linux-ath/ath5k/phy.c:286-344`.
pub const fn compute_ofdm_timings(freq_mhz: u16) -> Option<(u32, u32)> {
    if freq_mhz == 0 {
        return None;
    }
    // coef_scaled = ((5 * (40 << 24)) / 2) / freq_mhz = 1_677_721_600 / freq_mhz.
    let coef_scaled: u32 = 1_677_721_600u32 / (freq_mhz as u32);
    if coef_scaled == 0 {
        return None;
    }
    let ilog = 31u32 - coef_scaled.leading_zeros();
    if ilog == 0 || ilog > 24 {
        return None;
    }
    let coef_exp = 14u32 + 24u32 - ilog;
    if coef_exp < 16 || coef_exp >= 24 {
        return None;
    }
    let shift = 24u32 - coef_exp;
    let coef_man = coef_scaled + (1u32 << (shift - 1));
    let ds_coef_man = coef_man >> shift;
    let ds_coef_exp = coef_exp - 16;
    Some((ds_coef_man, ds_coef_exp))
}

/// Commit AR2413 2.4 GHz 11g EEPROM antenna switch tables (PCB T/R RF switch),
/// board-specific receiver/PA/LNA/IQ calibration, and AR5212A+ PHY tweaks per
/// `vendor/ref/linux-ath/ath5k/reset.c:848-885,976-1123` and `phy.c:2270-2281`.
/// Returns `Some((ob_1, db_1))` on success, or `None` if EEPROM reads or
/// checksum validation fail (no silent fallback).
unsafe fn ar2413_commit_eeprom_11g(base: u32) -> Option<(u8, u8)> {
    unsafe {
        // AR5212A+ PHY/MAC tweaks (reset.c:851-885).
        reg_w32(base, AR5K_PHY_ADC_CTL, 0x0002_A002);
        let dag = reg_r32(base, AR5K_PHY_DAG_CCK_CTL);
        reg_w32(
            base,
            AR5K_PHY_DAG_CCK_CTL,
            (dag & !(AR5K_PHY_DAG_CCK_CTL_EN_RSSI_THR | AR5K_PHY_DAG_CCK_CTL_RSSI_THR))
                | ((2u32 << AR5K_PHY_DAG_CCK_CTL_RSSI_THR_S) & AR5K_PHY_DAG_CCK_CTL_RSSI_THR),
        );
        reg_w32(base, AR5K_SEQ_MASK, 0x0000_000F);
        reg_w32(base, AR5K_PHY_BLUETOOTH, 0);
        let txcfg = reg_r32(base, AR5K_TXCFG);
        reg_w32(base, AR5K_TXCFG, txcfg & !AR5K_TXCFG_DCU_DBL_BUF_DIS);

        let ee_ver = eeprom_read(base, 0x00C1)?;
        if !ar2413_verify_eeprom_checksum(base, ee_ver) {
            return None;
        }
        let off = if ee_ver >= 0x3003 { 0x010Du32 } else { 0x00DAu32 };
        let mut w = [0u16; 5];
        for i in 0..5 {
            w[i] = eeprom_read(base, off + i as u32)?;
        }
        let ants = decode_eeprom_ants(w);

        let mut m = [0u16; 12];
        let mode_words = if ee_ver >= 0x4002 {
            12usize
        } else if ee_ver >= 0x4000 {
            11usize
        } else if ee_ver >= 0x3004 {
            7usize
        } else if ee_ver >= 0x3003 {
            6usize
        } else {
            5usize
        };
        for i in 0..mode_words {
            m[i] = eeprom_read(base, off + 5 + i as u32)?;
        }
        let modes = decode_eeprom_modes_11g(ee_ver, &m);

        // CCK-to-OFDM power/gain delta (reset.c:995-1007).
        let cck_pwr_delta = ((modes.cck_ofdm_power_delta as i32) * 2) / 10;
        let gain_neg = (-(modes.cck_ofdm_gain_delta as i32)) as u32;
        let pcdac_neg = (-cck_pwr_delta) as u32;
        reg_w32(
            base,
            AR5K_PHY_TX_PWR_ADJ,
            ((gain_neg << AR5K_PHY_TX_PWR_ADJ_CCK_GAIN_DELTA_S)
                & AR5K_PHY_TX_PWR_ADJ_CCK_GAIN_DELTA)
                | ((pcdac_neg << AR5K_PHY_TX_PWR_ADJ_CCK_PCDAC_INDEX_S)
                    & AR5K_PHY_TX_PWR_ADJ_CCK_PCDAC_INDEX),
        );

        // Program PCB RF antenna switch tables & idle control (phy.c:2270-2280).
        let ant_ctl = reg_r32(base, AR5K_PHY_ANT_CTL);
        reg_w32(
            base,
            AR5K_PHY_ANT_CTL,
            (ant_ctl & !AR5K_PHY_ANT_CTL_SWTABLE_IDLE)
                | ((ants.ant_idle << AR5K_PHY_ANT_CTL_SWTABLE_IDLE_S)
                    & AR5K_PHY_ANT_CTL_SWTABLE_IDLE)
                | AR5K_PHY_ANT_CTL_TXRX_EN,
        );
        reg_w32(base, AR5K_PHY_ANT_SWITCH_TABLE_0, ants.swtable_a);
        reg_w32(base, AR5K_PHY_ANT_SWITCH_TABLE_1, ants.swtable_b);

        // Noise floor threshold (reset.c:1022-1025).
        let nf_sval = (((modes.noise_floor_thr as i32) as u32) & 0x01FF) | (1 << 9);
        reg_w32(base, AR5K_PHY_NFTHRES, nf_sval);

        // Switch settling time & TX/RX attenuation (reset.c:1055-1062).
        let settling = reg_r32(base, AR5K_PHY_SETTLING);
        reg_w32(
            base,
            AR5K_PHY_SETTLING,
            (settling & !AR5K_PHY_SETTLING_SWITCH)
                | ((ants.switch_settling << AR5K_PHY_SETTLING_SWITCH_S) & AR5K_PHY_SETTLING_SWITCH),
        );
        let gain = reg_r32(base, AR5K_PHY_GAIN);
        reg_w32(
            base,
            AR5K_PHY_GAIN,
            (gain & !AR5K_PHY_GAIN_TXRX_ATTEN)
                | ((ants.atn_tx_rx << AR5K_PHY_GAIN_TXRX_ATTEN_S) & AR5K_PHY_GAIN_TXRX_ATTEN),
        );

        // ADC / PGA desired levels (reset.c:1064-1071).
        let des = reg_r32(base, AR5K_PHY_DESIRED_SIZE);
        reg_w32(
            base,
            AR5K_PHY_DESIRED_SIZE,
            (des & !(AR5K_PHY_DESIRED_SIZE_ADC | AR5K_PHY_DESIRED_SIZE_PGA))
                | ((modes.adc_desired_size as u32) & AR5K_PHY_DESIRED_SIZE_ADC)
                | (((modes.pga_desired_size as u32) << AR5K_PHY_DESIRED_SIZE_PGA_S)
                    & AR5K_PHY_DESIRED_SIZE_PGA),
        );

        // TX/RX margin (reset.c:1074-1077).
        if ee_ver >= 0x4001 {
            let g2 = reg_r32(base, AR5K_PHY_GAIN_2GHZ);
            reg_w32(
                base,
                AR5K_PHY_GAIN_2GHZ,
                (g2 & !AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX)
                    | (((modes.margin_tx_rx as u32) << AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX_S)
                        & AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX),
            );
        }

        // External PA (XPA) enable/disable turnaround timing (reset.c:1080-1085).
        let xpa_dis = modes.tx_end2xpa_disable as u32;
        let xpa_en = modes.tx_frm2xpa_enable as u32;
        reg_w32(
            base,
            AR5K_PHY_RF_CTL4,
            (xpa_dis << 24) | (xpa_dis << 16) | (xpa_en << 8) | xpa_en,
        );

        // External LNA (XLNA) turnaround timing (reset.c:1087-1090).
        let ctl3 = reg_r32(base, AR5K_PHY_RF_CTL3);
        reg_w32(
            base,
            AR5K_PHY_RF_CTL3,
            (ctl3 & !AR5K_PHY_RF_CTL3_TXE2XLNA_ON)
                | (((modes.tx_end2xlna_enable as u32) << AR5K_PHY_RF_CTL3_TXE2XLNA_ON_S)
                    & AR5K_PHY_RF_CTL3_TXE2XLNA_ON),
        );

        // Thresh62 (reset.c:1092-1095).
        let nf = reg_r32(base, AR5K_PHY_NF);
        reg_w32(
            base,
            AR5K_PHY_NF,
            (nf & !AR5K_PHY_NF_THRESH62)
                | (((modes.thr_62 as u32) << AR5K_PHY_NF_THRESH62_S) & AR5K_PHY_NF_THRESH62),
        );

        // I/Q correction coefficients — enable bit set last (reset.c:1110-1118).
        if ee_ver >= 0x4000 {
            let iq = reg_r32(base, AR5K_PHY_IQ);
            let iq = (iq & !(AR5K_PHY_IQ_CORR_Q_I_COFF | AR5K_PHY_IQ_CORR_Q_Q_COFF))
                | (((modes.i_cal as u32) << AR5K_PHY_IQ_CORR_Q_I_COFF_S)
                    & AR5K_PHY_IQ_CORR_Q_I_COFF)
                | ((modes.q_cal as u32) & AR5K_PHY_IQ_CORR_Q_Q_COFF);
            reg_w32(base, AR5K_PHY_IQ, iq);
            reg_w32(base, AR5K_PHY_IQ, iq | AR5K_PHY_IQ_CORR_ENABLE);
        }

        // Disable heavy clipping on EEPROM >= 5.1 (reset.c:1120-1122).
        if ee_ver >= 0x5001 {
            reg_w32(base, AR5K_PHY_HEAVY_CLIP_ENABLE, 0);
        }
        Some((modes.ob_1, modes.db_1))
    }
}

unsafe fn ar2413_radio_init_11g(base: u32, freq_mhz: u16) -> bool {
    let Some((rf_lo, rf_hi)) = rf5112_channel_regs(freq_mhz) else {
        return false;
    };
    if !unsafe { wake_chip(base) } {
        return false;
    }
    if !unsafe {
        nic_reset(
            base,
            AR5K_RESET_CTL_PCU | AR5K_RESET_CTL_BASEBAND | AR5K_RESET_CTL_PCI,
        )
    } {
        return false;
    }
    if !unsafe { wake_chip(base) } {
        return false;
    }
    if !unsafe { nic_reset(base, 0) } {
        return false;
    }
    unsafe {
        let pcfg = reg_r32(base, AR5K_PCICFG);
        reg_w32(base, AR5K_PCICFG, pcfg | AR5K_PCICFG_RETRY_FIX);

        // 2.4 GHz 802.11g PLL + PHY mode (ath5k/reset.c:747-826).
        reg_w32(base, AR5K_PHY_PLL, AR5K_PHY_PLL_RF5112_44MHZ);
        cardbus::delay_ms(1);
        reg_w32(base, AR5K_PHY_MODE, AR5K_PHY_MODE_11G_2GHZ);
        reg_w32(base, AR5K_PHY_TURBO, 0);

        // PHY access + AR2413 802.11g initvals + EEPROM RF switch/PHY calibration + RF2413 gain + RF banks.
        reg_w32(base, AR5K_PHY_BASE, AR5K_PHY_SHIFT_5GHZ);
        write_reg_table(base, AR2413_11G_INITVALS);
        let Some((ob_1, db_1)) = ar2413_commit_eeprom_11g(base) else {
            ath_state().last_fail = 4;
            return false;
        };
        // Write channel-specific OFDM pilot-tracking delta slope (phy.c:286-344,3799).
        if let Some((dsc_man, dsc_exp)) = compute_ofdm_timings(freq_mhz) {
            let t3 = reg_r32(base, AR5K_PHY_TIMING_3);
            reg_w32(
                base,
                AR5K_PHY_TIMING_3,
                (t3 & !(AR5K_PHY_TIMING_3_DSC_MAN | AR5K_PHY_TIMING_3_DSC_EXP))
                    | ((dsc_man << AR5K_PHY_TIMING_3_DSC_MAN_S) & AR5K_PHY_TIMING_3_DSC_MAN)
                    | ((dsc_exp << AR5K_PHY_TIMING_3_DSC_EXP_S) & AR5K_PHY_TIMING_3_DSC_EXP),
            );
        }
        write_reg_table(base, RF2413_2GHZ_RFGAIN);
        cardbus::delay_ms(1);
        // Patch EEPROM 2.4 GHz OB/DB bias into Bank 6 entries 23..=24 before upload (phy.c:935-940).
        let (p23, p24) = patch_rf2413_ob_db(
            RF2413_11G_RFBANKS[23].1,
            RF2413_11G_RFBANKS[24].1,
            ob_1,
            db_1,
        );
        for (i, &(reg, val)) in RF2413_11G_RFBANKS.iter().enumerate() {
            if (i & 7) == 7 {
                let _ = reg_r32(base, AR5K_SREV);
            }
            let out_val = if i == 23 {
                p23
            } else if i == 24 {
                p24
            } else {
                val
            };
            reg_w32(base, reg as usize, out_val);
        }

        // Program channel frequency and activate PHY.
        reg_w32(base, AR5K_RF_BUFFER, rf_lo);
        reg_w32(base, AR5K_RF_BUFFER_CONTROL_5, rf_hi);
        reg_w32(base, AR5K_PHY_ACT, AR5K_PHY_ACT_ENABLE);
        cardbus::delay_ms(5);

        // ADC ready check (ath5k/phy.c:3896-3903).
        let tst1 = reg_r32(base, AR5K_PHY_TST1);
        reg_w32(base, AR5K_PHY_TST1, AR5K_PHY_TST1_TXHOLD);
        let mut adc_ok = false;
        for _ in 0..20 {
            if (reg_r32(base, AR5K_PHY_ADC_TEST) & 0x10) == 0 {
                adc_ok = true;
                break;
            }
            cardbus::delay_ms(1);
        }
        reg_w32(base, AR5K_PHY_TST1, tst1);
        if !adc_ok {
            ath_state().last_fail = 5;
            return false;
        }

        // AGC + noise-floor calibration (ath5k/phy.c:3924-3944).
        let agc = reg_r32(base, AR5K_PHY_AGCCTL);
        reg_w32(
            base,
            AR5K_PHY_AGCCTL,
            agc | AR5K_PHY_AGCCTL_CAL | AR5K_PHY_AGCCTL_NF,
        );
        let mut agc_ok = false;
        for _ in 0..50 {
            if (reg_r32(base, AR5K_PHY_AGCCTL) & AR5K_PHY_AGCCTL_CAL) == 0 {
                agc_ok = true;
                break;
            }
            cardbus::delay_ms(1);
        }
        if !agc_ok {
            ath_state().last_fail = 6;
            return false;
        }

        // Configure RX filters (beacons + broadcast + multicast + unicast) and enable RX.
        reg_w32(base, AR5K_MCAST_FILTER0_5211, 0xFFFF_FFFF);
        reg_w32(base, AR5K_MCAST_FILTER1_5211, 0xFFFF_FFFF);
        reg_w32(
            base,
            AR5K_RX_FILTER_5211,
            AR5K_RX_FILTER_BEACON
                | AR5K_RX_FILTER_BCAST
                | AR5K_RX_FILTER_MCAST
                | AR5K_RX_FILTER_UCAST,
        );
        let diag = reg_r32(base, AR5K_DIAG_SW_5211);
        reg_w32(base, AR5K_DIAG_SW_5211, diag & !AR5K_DIAG_SW_DIS_RX_5211);
        reg_w32(base, AR5K_RXCFG, 0x0005); // 128-byte DMA burst
    }
    true
}

unsafe fn ar2413_set_channel_2g(base: u32, freq_mhz: u16) {
    let Some((rf_lo, rf_hi)) = rf5112_channel_regs(freq_mhz) else {
        return;
    };
    unsafe {
        reg_w32(base, AR5K_RF_BUFFER, rf_lo);
        reg_w32(base, AR5K_RF_BUFFER_CONTROL_5, rf_hi);
        cardbus::delay_ms(3);
        let agc = reg_r32(base, AR5K_PHY_AGCCTL);
        reg_w32(
            base,
            AR5K_PHY_AGCCTL,
            agc | AR5K_PHY_AGCCTL_CAL | AR5K_PHY_AGCCTL_NF,
        );
        for _ in 0..15 {
            if (reg_r32(base, AR5K_PHY_AGCCTL) & AR5K_PHY_AGCCTL_CAL) == 0 {
                break;
            }
            cardbus::delay_ms(1);
        }
    }
}

/// FFI entry for JLM V86 API `AX=0x000B`: passive 2.4 GHz beacon scan across
/// channels 1..=11 using caller-supplied 16 KiB DOS DMA block `dma_seg`.
/// Copies the 720-byte `WifiScanResult` into `dst_lin`.
/// Returns `ap_count` (0..=16).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_wifi_scan(dma_seg: u32, dst_lin: u32) -> u32 {
    // Scanning reprograms the radio end to end; run against a live session
    // it would silently tear the session down, the same trap as probing.
    // Report an empty scan instead and leave the hardware alone.
    if ath_state().connected {
        if dst_lin != 0 {
            let res = WifiScanResult::ZERO;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    &res as *const WifiScanResult as *const u8,
                    dst_lin as *mut u8,
                    core::mem::size_of::<WifiScanResult>(),
                );
            }
        }
        return 0;
    }
    let st = ath_state();
    st.scan_res = WifiScanResult::ZERO;
    let res = &mut st.scan_res;
    let Some((slot, _did, bar0_phys)) = cardbus::find_card_bar0(ATHEROS_VID) else {
        if dst_lin != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    res as *const WifiScanResult as *const u8,
                    dst_lin as *mut u8,
                    core::mem::size_of::<WifiScanResult>(),
                );
            }
        }
        return 0;
    };
    res.present = 1;
    res.stage = 1;

    // Map BAR0 (reusing cached mapping if present).
    let bar0_lin = if st.mapped_phys == bar0_phys && st.mapped_lin != 0 {
        st.mapped_lin
    } else {
        let lin = unsafe { cardbus::map_mmio(bar0_phys, 16) };
        if lin != 0 {
            st.mapped_phys = bar0_phys;
            st.mapped_lin = lin;
        }
        lin
    };
    if bar0_lin == 0 {
        if dst_lin != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    res as *const WifiScanResult as *const u8,
                    dst_lin as *mut u8,
                    core::mem::size_of::<WifiScanResult>(),
                );
            }
        }
        return 0;
    }
    res.stage = 2;

    // A bad cold boot can leave the card in the same late-reset state the
    // probe path heals (SREV all-ones). Radio init against that state
    // programs a BAR nobody answers on, and a first-run WSCAN failed
    // silently where WCONN — which goes through probe_internal — would
    // have recovered. Run the same single heal attempt before trusting
    // the BAR; if the card still does not answer, fail at stage 2.
    let srev = unsafe { reg_r32(bar0_lin, AR5K_SREV) };
    let srev = unsafe { srev_heal_once(slot, bar0_lin, srev) };
    if srev == 0xFFFF_FFFF || srev == 0 {
        if dst_lin != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    res as *const WifiScanResult as *const u8,
                    dst_lin as *mut u8,
                    core::mem::size_of::<WifiScanResult>(),
                );
            }
        }
        return 0;
    }

    // Validate caller's 16 KiB DMA block via Jemm VDMAD (PCI bus-master, no 64 KiB ISA limit).
    let dma_lin = dma_seg << 4;
    let Some(dma_phys) = (unsafe { vdmad_lock_dma_region_pci(dma_lin, WIFI_DMA_SIZE as u32) }) else {
        if dst_lin != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    res as *const WifiScanResult as *const u8,
                    dst_lin as *mut u8,
                    core::mem::size_of::<WifiScanResult>(),
                );
            }
        }
        return 0;
    };
    res.stage = 3;

    // Bring up AR2413 802.11g radio on channel 1 (2412 MHz).
    if !unsafe { ar2413_radio_init_11g(bar0_lin, 2412) } {
        unsafe { vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32) };
        if dst_lin != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    res as *const WifiScanResult as *const u8,
                    dst_lin as *mut u8,
                    core::mem::size_of::<WifiScanResult>(),
                );
            }
        }
        return 0;
    }
    res.stage = 4;

    // Passive scan channels 1..=11 (150 ms dwell each -> ~1.7 s total).
    for ch in 1u8..=11u8 {
        let freq = 2407u16 + 5u16 * (ch as u16);
        unsafe { stop_rx_dma(bar0_lin) };
        if ch > 1 {
            unsafe { ar2413_set_channel_2g(bar0_lin, freq) };
        }
        unsafe {
            init_rx_ring(dma_lin, dma_phys);
            reg_w32(bar0_lin, AR5K_RXDP, dma_phys);
            reg_w32(bar0_lin, AR5K_CR, AR5K_CR_RXE);
            let _ = reg_r32(bar0_lin, AR5K_CR);
        }

        let mut idx = 0usize;
        for _tick in 0..150u32 {
            cardbus::delay_ms(1);
            // Scan the whole ring each tick, like the data path does: a
            // completed descriptor can sit behind an unfinished one, and
            // stopping at the first gap strands it until the hardware
            // wraps back around to that slot.
            let start = idx;
            for step in 0..RX_DESC_COUNT {
                let cur = (start + step) % RX_DESC_COUNT;
                let doff = cur * RX_DESC_STRIDE;
                let stat1 = unsafe { dma_r32(dma_lin, doff + 20) };
                if (stat1 & AR5K_RX_STAT1_DONE) == 0 {
                    continue;
                }
                let stat0 = unsafe { dma_r32(dma_lin, doff + 16) };
                let dec = decode_5212_rx_status(stat0, stat1);
                res.rx_total = res.rx_total.wrapping_add(1);
                if dec.ok && dec.len >= 36 {
                    res.rx_ok = res.rx_ok.wrapping_add(1);
                    let flen = (dec.len as usize).min(RX_BUF_SIZE);
                    let buf_ptr = (dma_lin as usize + RX_BUF_BASE + cur * RX_BUF_SIZE) as *const u8;
                    let frame = unsafe { core::slice::from_raw_parts(buf_ptr, flen) };
                    if let Some(beacon) = wifi_core::frame::parse_beacon(frame) {
                        res.beacons = res.beacons.wrapping_add(1);
                        res.record_beacon(&beacon, ch, dec.rssi);
                    }
                }
                // Recycle descriptor cleanly.
                unsafe {
                    dma_w32(dma_lin, doff + 12, RX_BUF_SIZE as u32);
                    dma_w32(dma_lin, doff + 16, 0);
                    dma_w32(dma_lin, doff + 20, 0);
                }
                idx = (cur + 1) % RX_DESC_COUNT;
            }
        }
    }

    unsafe {
        stop_rx_dma(bar0_lin);
        vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32);
    }
    res.stage = 5;

    if dst_lin != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                res as *const WifiScanResult as *const u8,
                dst_lin as *mut u8,
                core::mem::size_of::<WifiScanResult>(),
            );
        }
    }
    res.ap_count as u32
}

// ---------------------------------------------------------------------
// Gate 4b Step 4b.5 — AR2413 TX QCU/DCU 0, 4-word 5212 TX descriptor,
// hardware AES-CCM key cache, STA/BSSID/AID, and shared IRQ 11 helpers.
// ---------------------------------------------------------------------

/// AR5212/AR2413 4-word TX control descriptor words (`desc.h:213-269`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TxDesc4W {
    pub ctl0: u32,
    pub ctl1: u32,
    pub ctl2: u32,
    pub ctl3: u32,
}

/// Build the 4-word AR5212/AR2413 TX control descriptor words matching
/// `ath5k_hw_setup_4word_tx_desc` (`vendor/ref/linux-ath/ath5k/desc.c:241-359`).
///
/// - `frame_len` is the actual DMA buffer byte length (`CTL1.BUF_LEN`).
/// - `CTL0.FRAME_LEN` includes the 4-byte hardware FCS plus the 8-byte CCMP
///   MIC when `key_idx.is_some()`.
/// - `tx_power = 60` (0.5 dB steps), `tries0 = 4`, `rate0 = rate_code`
///   (falls back to `ATH5K_RATE_CODE_1M` if `rate_code & 0x1F == 0` to avoid
///   continuous-noise HW lockup per `desc.c:275`).
pub const fn build_5212_tx_ctl(
    frame_len: usize,
    rate_code: u8,
    key_idx: Option<u8>,
    no_ack: bool,
) -> TxDesc4W {
    let mic_len = if key_idx.is_some() { 8usize } else { 0usize };
    // Per ath5k/base.c:826 + desc.c:294,310: hardware CCMP expects BOTH
    // CTL0.FRAME_LEN (+8 MIC +4 FCS) and CTL1.BUF_LEN (+8 MIC placeholder).
    let total_frame_len = (frame_len + mic_len + 4) as u32;
    let buf_len = (frame_len + mic_len) as u32;
    let tx_power = 60u32; // AR5K_TUNE_MAX_TXPOWER (TPC disabled; RATE3/4 cap CCK at 16 dBm)
    let tries0 = 10u32;
    let safe_rate = if (rate_code & 0x1F) == 0 {
        ATH5K_RATE_CODE_1M
    } else {
        rate_code
    };

    let mut ctl0 = (total_frame_len & AR5K_4W_TX_DESC_CTL0_FRAME_LEN)
        | ((tx_power << AR5K_4W_TX_DESC_CTL0_XMIT_POWER_S) & AR5K_4W_TX_DESC_CTL0_XMIT_POWER)
        | AR5K_4W_TX_DESC_CTL0_CLRDMASK;
    let mut ctl1 = buf_len & AR5K_4W_TX_DESC_CTL1_BUF_LEN;

    if let Some(kidx) = key_idx {
        ctl0 |= AR5K_4W_TX_DESC_CTL0_ENCRYPT_KEY_VALID;
        ctl1 |= ((kidx as u32) << AR5K_4W_TX_DESC_CTL1_ENCRYPT_KEY_IDX_S)
            & AR5K_4W_TX_DESC_CTL1_ENCRYPT_KEY_IDX;
    }
    if no_ack {
        ctl1 |= AR5K_4W_TX_DESC_CTL1_NOACK;
    }

    let mut ctl2;
    let mut ctl3;
    if safe_rate == ATH5K_RATE_CODE_36M {
        // 4-stage hardware Multi-Rate Retry (MRR) ladder for 802.11g data frames:
        // 36M OFDM (3 tries) -> 18M OFDM (3 tries) -> 6M OFDM (3 tries) -> 1M CCK (5 tries).
        ctl2 = ((3u32 << AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0_S) & AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0)
            | ((3u32 << AR5K_4W_TX_DESC_CTL2_XMIT_TRIES1_S) & AR5K_4W_TX_DESC_CTL2_XMIT_TRIES1)
            | ((3u32 << AR5K_4W_TX_DESC_CTL2_XMIT_TRIES2_S) & AR5K_4W_TX_DESC_CTL2_XMIT_TRIES2)
            | ((5u32 << AR5K_4W_TX_DESC_CTL2_XMIT_TRIES3_S) & AR5K_4W_TX_DESC_CTL2_XMIT_TRIES3);
        ctl3 = ((ATH5K_RATE_CODE_36M as u32) & AR5K_4W_TX_DESC_CTL3_XMIT_RATE0)
            | (((ATH5K_RATE_CODE_18M as u32) & 0x1F) << AR5K_4W_TX_DESC_CTL3_XMIT_RATE1_S)
            | (((ATH5K_RATE_CODE_6M as u32) & 0x1F) << AR5K_4W_TX_DESC_CTL3_XMIT_RATE2_S)
            | (((ATH5K_RATE_CODE_1M as u32) & 0x1F) << AR5K_4W_TX_DESC_CTL3_XMIT_RATE3_S);
    } else {
        ctl2 = (tries0 << AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0_S) & AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0;
        ctl3 = (safe_rate as u32) & AR5K_4W_TX_DESC_CTL3_XMIT_RATE0;
        // Hardware multi-rate fallback: if a higher rate (e.g. 6M OFDM) fails to
        // get an ACK, automatically fall back to 7 retries at 1M CCK (0x1B).
        if safe_rate != ATH5K_RATE_CODE_1M {
            ctl2 |= (7u32 << AR5K_4W_TX_DESC_CTL2_XMIT_TRIES1_S) & AR5K_4W_TX_DESC_CTL2_XMIT_TRIES1;
            ctl3 |= ((ATH5K_RATE_CODE_1M as u32) & 0x1F) << AR5K_4W_TX_DESC_CTL3_XMIT_RATE1_S;
        }
    }

    TxDesc4W {
        ctl0,
        ctl1,
        ctl2,
        ctl3,
    }
}

/// Encode a 16-byte AES-CCM (WPA2 CCMP) key and optional peer MAC address into
/// the exact 8-dword AR5212/AR2413 hardware key-cache entry layout per
/// `vendor/ref/linux-ath/reg.h:42-63` and `vendor/ref/linux-ath/key.c:87-189,318-334`:
///
/// - `words[0]` (`KEY0`, `+0x00`): `key[0..4]` little-endian
/// - `words[1]` (`KEY1`, `+0x04`): `key[4..6]` little-endian (zero-extended)
/// - `words[2]` (`KEY2`, `+0x08`): `key[6..10]` little-endian
/// - `words[3]` (`KEY3`, `+0x0C`): `key[10..12]` little-endian (zero-extended)
/// - `words[4]` (`KEY4`, `+0x10`): `key[12..16]` little-endian
/// - `words[5]` (`TYPE`, `+0x14`): `AR5K_KEYTABLE_TYPE_CCM` (`6`, AES-CCM)
/// - `words[6]` (`MAC0`, `+0x18`): `(mac_le32 >> 1) | ((mac_le16 & 1) << 31)` (or `0`)
/// - `words[7]` (`MAC1`, `+0x1C`): `(mac_le16 >> 1) | unicast_valid`
pub const fn encode_ccmp_key_words(key: &[u8; 16], mac: Option<&[u8; 6]>) -> [u32; 8] {
    let key0 = u32::from_le_bytes([key[0], key[1], key[2], key[3]]);
    let key1 = u16::from_le_bytes([key[4], key[5]]) as u32;
    let key2 = u32::from_le_bytes([key[6], key[7], key[8], key[9]]);
    let key3 = u16::from_le_bytes([key[10], key[11]]) as u32;
    let key4 = u32::from_le_bytes([key[12], key[13], key[14], key[15]]);
    let key_type = AR5K_KEYTABLE_TYPE_CCM;

    let (mac_lo, mac_hi, valid_flag) = match mac {
        Some(m) => {
            let unicast_flag = if (m[0] & 0x01) != 0 {
                0
            } else {
                AR5K_KEYTABLE_VALID
            };
            let raw_lo = u32::from_le_bytes([m[0], m[1], m[2], m[3]]);
            let raw_hi = u16::from_le_bytes([m[4], m[5]]) as u32;
            let lo = (raw_lo >> 1) | ((raw_hi & 1) << 31);
            let hi = raw_hi >> 1;
            (lo, hi, unicast_flag)
        }
        None => (0, 0, AR5K_KEYTABLE_VALID),
    };

    [
        key0,
        key1,
        key2,
        key3,
        key4,
        key_type,
        mac_lo,
        mac_hi | valid_flag,
    ]
}

/// Initialize AR2413 QCU 0 + DCU 0 for data and management TX per
/// `vendor/ref/linux-ath/ath5k/qcu.c` and `dma.c`.
#[allow(dead_code)]
pub(crate) unsafe fn ar2413_init_tx_q0(base: u32) {
    unsafe {
        // Stop QCU 0 cleanly if currently enabled.
        reg_w32(base, AR5K_QCU_TXD, 1);
        for _ in 0..20 {
            if (reg_r32(base, AR5K_QCU_TXE) & 1) == 0 {
                break;
            }
            cardbus::delay_ms(1);
        }
        reg_w32(base, AR5K_QCU_TXD, 0);

        // Configure TX DMA burst = 128B and FIFO trigger level = 64B (dma.c:857-858).
        let txcfg = reg_r32(base, AR5K_TXCFG);
        reg_w32(
            base,
            AR5K_TXCFG,
            (txcfg & !(AR5K_TXCFG_SDMAMR | AR5K_TXCFG_TXFULL))
                | AR5K_TXCFG_SDMAMR_128B
                | AR5K_TXCFG_TXFULL_64B,
        );

        // DCU 0 local IFS: cw_min=15, cw_max=1023, aifs=2 (qcu.c:340-344).
        let lcl_ifs = 15u32 | (1023u32 << 10) | (2u32 << 20);
        reg_w32(base, AR5K_DCU_LCL_IFS_0, lcl_ifs);

        // DCU 0 retry limits: rts=4, sta_rts=4, sta_data=14 (covers 4-stage MRR ladder).
        let retry_lmt = 4u32 | (4u32 << 8) | (14u32 << 14);
        reg_w32(base, AR5K_DCU_RETRY_LMT_0, retry_lmt);

        // QCU 0 misc: ASAP scheduling; DCU 0 misc: wait for next fragment (qcu.c:357-358).
        reg_w32(base, AR5K_QCU_MISC_0, 0);
        reg_w32(base, AR5K_DCU_MISC_0, AR5K_DCU_MISC_FRAG_WAIT);

        // Map QCU 0 -> DCU 0 (qcu.c:539) and clear TXDP(0).
        reg_w32(base, AR5K_DCU_QCUMASK_0, 1);
        reg_w32(base, AR5K_QUEUE_TXDP_0, 0);
    }
}

/// Program station MAC, BSSID, AID, BSSID mask, and STA key-search mode per
/// `vendor/ref/linux-ath/ath5k/pcu.c:371-430,877-943,1002-1009`.
#[allow(dead_code)]
pub(crate) unsafe fn ar2413_set_sta_bssid(
    base: u32,
    sta_mac: &[u8; 6],
    bssid: &[u8; 6],
    aid: u16,
) {
    let sta_lo = u32::from_le_bytes([sta_mac[0], sta_mac[1], sta_mac[2], sta_mac[3]]);
    let sta_hi = u16::from_le_bytes([sta_mac[4], sta_mac[5]]) as u32;
    let bss_lo = u32::from_le_bytes([bssid[0], bssid[1], bssid[2], bssid[3]]);
    let bss_hi = u16::from_le_bytes([bssid[4], bssid[5]]) as u32;

    unsafe {
        let mut pcu_reg = reg_r32(base, AR5K_STA_ID1) & 0xFFFF_0000;
        pcu_reg &= !(AR5K_STA_ID1_AP
            | AR5K_STA_ID1_ADHOC
            | AR5K_STA_ID1_PWR_SV
            | AR5K_STA_ID1_NO_KEYSRCH
            | AR5K_STA_ID1_DEFAULT_ANTENNA
            | AR5K_STA_ID1_DESC_ANTENNA
            | AR5K_STA_ID1_RTS_DEF_ANTENNA
            | AR5K_STA_ID1_SELFGEN_DEF_ANT);
        // Use Linux ath5k AR5K_ANTMODE_DEFAULT (phy.c:2310-2317) now that the
        // PCB's EEPROM RF antenna switch tables (0x9960/0x9964) are programmed.
        pcu_reg |= AR5K_STA_ID1_KEYSRCH_MODE
            | AR5K_STA_ID1_BASE_RATE_11B
            | AR5K_STA_ID1_ACKCTS_6MB;

        let fast_div = reg_r32(base, AR5K_PHY_FAST_ANT_DIV);
        reg_w32(base, AR5K_PHY_FAST_ANT_DIV, fast_div | AR5K_PHY_FAST_ANT_DIV_EN);
        let agc = reg_r32(base, AR5K_PHY_AGCCTL);
        reg_w32(base, AR5K_PHY_AGCCTL, agc & !AR5K_PHY_AGCCTL_OFDM_DIV_DIS);
        reg_w32(base, AR5K_DEFAULT_ANTENNA, 1);

        reg_w32(base, AR5K_STA_ID0, sta_lo);
        reg_w32(base, AR5K_STA_ID1, pcu_reg | sta_hi);

        // Match all 48 bits of our STA MAC for hardware ACK generation.
        reg_w32(base, AR5K_BSSMSKL, 0xFFFF_FFFF);
        reg_w32(base, AR5K_BSSMSKU, 0x0000_FFFF);

        reg_w32(base, AR5K_BSS_ID0, bss_lo);
        reg_w32(
            base,
            AR5K_BSS_ID1,
            bss_hi | (((aid as u32) & 0x3FFF) << AR5K_BSS_ID1_AID_S),
        );

        // Ensure hardware encryption/decryption engines are enabled.
        let diag = reg_r32(base, AR5K_DIAG_SW_5211);
        reg_w32(
            base,
            AR5K_DIAG_SW_5211,
            diag & !(AR5K_DIAG_SW_DIS_ENC | AR5K_DIAG_SW_DIS_DEC),
        );
    }
}

/// Clear all 128 AR2413 hardware key-cache slots (`AR5K_KEYTABLE_TYPE_CLR`)
/// per `vendor/ref/linux-ath/key.c:42-64`.
#[allow(dead_code)]
pub(crate) unsafe fn ar2413_clear_keycache(base: u32) {
    for slot in 0..AR5K_KEYTABLE_SIZE {
        let off = AR5K_KEYTABLE_0 + slot * 32;
        unsafe {
            reg_w32(base, off + 0, 0);
            reg_w32(base, off + 4, 0);
            reg_w32(base, off + 8, 0);
            reg_w32(base, off + 12, 0);
            reg_w32(base, off + 16, 0);
            reg_w32(base, off + 20, AR5K_KEYTABLE_TYPE_CLR);
            reg_w32(base, off + 24, 0);
            reg_w32(base, off + 28, 0);
        }
    }
}

/// Program one 8-dword AES-CCM (WPA2 CCMP) key-cache slot in paired 32-bit
/// register order per `vendor/ref/linux-ath/key.c:192-197,318-334`.
#[allow(dead_code)]
pub(crate) unsafe fn ar2413_install_ccmp_key(
    base: u32,
    slot: u8,
    key: &[u8; 16],
    mac: Option<&[u8; 6]>,
) {
    if (slot as usize) >= AR5K_KEYTABLE_SIZE {
        return;
    }
    let words = encode_ccmp_key_words(key, mac);
    let off = AR5K_KEYTABLE_0 + (slot as usize) * 32;
    let mut i = 0usize;
    while i < 8 {
        unsafe { reg_w32(base, off + i * 4, words[i]) };
        i += 1;
    }
    let _ = unsafe { reg_r32(base, AR5K_SREV) };
}

/// Transmit a single 802.11 frame on QCU 0 using a 4-word 5212 TX descriptor
/// and poll `tx_status_1 & AR5K_DESC_TX_STATUS1_DONE` up to 50 ms.
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn ar2413_tx_frame_polled(
    base: u32,
    tx_desc_lin: u32,
    tx_desc_phys: u32,
    tx_buf_lin: u32,
    tx_buf_phys: u32,
    frame: &[u8],
    rate_code: u8,
    key_idx: Option<u8>,
    no_ack: bool,
) -> bool {
    let mic_len = if key_idx.is_some() { 8usize } else { 0usize };
    if frame.is_empty()
        || frame.len() + mic_len > 2048
        || tx_desc_lin == 0
        || tx_desc_phys == 0
        || tx_buf_lin == 0
        || tx_buf_phys == 0
        || (rate_code & 0x1F) == 0
    {
        return false;
    }

    unsafe {
        // Ensure QCU 0 is idle before writing TXDP(0) (dma.c:424-425).
        for _ in 0..10 {
            if (reg_r32(base, AR5K_QCU_TXE) & 1) == 0 {
                break;
            }
            cardbus::delay_ms(1);
        }
        if (reg_r32(base, AR5K_QCU_TXE) & 1) != 0 {
            return false;
        }

        // Copy frame payload into physically contiguous TX DMA bounce buffer
        // and zero-fill the 8-byte CCMP MIC trailer space when encrypting.
        core::ptr::copy_nonoverlapping(frame.as_ptr(), tx_buf_lin as *mut u8, frame.len());
        if mic_len > 0 {
            core::ptr::write_bytes((tx_buf_lin as usize + frame.len()) as *mut u8, 0, mic_len);
        }

        // Populate 8-dword AR5212 4-word TX descriptor (desc.h:326-358).
        let ctl = build_5212_tx_ctl(frame.len(), rate_code, key_idx, no_ack);
        dma_w32(tx_desc_lin, 0, 0); // ds_link
        dma_w32(tx_desc_lin, 4, tx_buf_phys); // ds_data
        dma_w32(tx_desc_lin, 8, ctl.ctl0); // tx_control_0
        dma_w32(tx_desc_lin, 12, ctl.ctl1); // tx_control_1
        dma_w32(tx_desc_lin, 16, ctl.ctl2); // tx_control_2
        dma_w32(tx_desc_lin, 20, ctl.ctl3); // tx_control_3
        dma_w32(tx_desc_lin, 24, 0); // tx_status_0
        dma_w32(tx_desc_lin, 28, 0); // tx_status_1

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);

        // Arm QCU 0 and trigger TX DMA.
        reg_w32(base, AR5K_QUEUE_TXDP_0, tx_desc_phys);
        reg_w32(base, AR5K_QCU_TXE, 1);
        let _ = reg_r32(base, AR5K_QCU_TXE);

        // Poll tx_status_1.DONE: spin briefly (~500 us via port 0x80) so fast
        // 36M/18M OFDM frames return in ~150-450 us instead of sleeping 1 ms,
        // then fall back to 1 ms polling up to 100 ms for 1M fallback retries.
        let st = ath_state();
        st.tx_total = st.tx_total.wrapping_add(1);
        for iter in 0..600u32 {
            let stat1 = dma_r32(tx_desc_lin, 28);
            if (stat1 & AR5K_DESC_TX_STATUS1_DONE) != 0 {
                let stat0 = dma_r32(tx_desc_lin, 24);
                st.tx_last_stat0 = stat0;
                st.tx_last_stat1 = stat1;
                st.tx_retries_sum = st.tx_retries_sum.wrapping_add((stat0 >> 8) & 0x0F);
                if (stat0 & AR5K_DESC_TX_STATUS0_FRAME_XMIT_OK) != 0 {
                    st.tx_ok = st.tx_ok.wrapping_add(1);
                    return true;
                }
                if (stat0 & 0x0000_0002) != 0 {
                    st.tx_fail_retry = st.tx_fail_retry.wrapping_add(1);
                }
                if (stat0 & 0x0000_0004) != 0 {
                    st.tx_fail_fifo = st.tx_fail_fifo.wrapping_add(1);
                }
                if (stat0 & 0x0000_0008) != 0 {
                    st.tx_fail_filt = st.tx_fail_filt.wrapping_add(1);
                }
                return false;
            }
            if iter < 500 {
                let _ = inb(0x80);
            } else {
                cardbus::delay_ms(1);
            }
        }
        st.tx_timeout = st.tx_timeout.wrapping_add(1);
        // The frame never completed. Merely reporting failure would leave
        // QCU 0 armed on a descriptor the hardware is never going to
        // finish, and every later transmission would then fail the idle
        // pre-check for the rest of the session. Disable the queue the way
        // ar2413_init_tx_q0 does and clear TXDP, so the next call re-points
        // TXDP at a fresh descriptor and starts from a known-idle queue.
        reg_w32(base, AR5K_QCU_TXD, 1);
        for _ in 0..20 {
            if (reg_r32(base, AR5K_QCU_TXE) & 1) == 0 {
                break;
            }
            cardbus::delay_ms(1);
        }
        reg_w32(base, AR5K_QCU_TXD, 0);
        reg_w32(base, AR5K_QUEUE_TXDP_0, 0);
    }
    false
}

/// Enable or disable AR2413 `RXOK` hardware interrupts cleanly per
/// `vendor/ref/linux-ath/ath5k/dma.c:740-822`.
#[allow(dead_code)]
pub(crate) unsafe fn ar2413_set_irq_rx(base: u32, enable: bool) {
    unsafe {
        reg_w32(base, AR5K_IER, AR5K_IER_DISABLE);
        let _ = reg_r32(base, AR5K_IER);

        // Clear any stale primary interrupt status bits.
        let _ = reg_r32(base, AR5K_RAC_PISR);
        reg_w32(base, AR5K_PISR, 0xFFFF_FFFF);
        let _ = reg_r32(base, AR5K_PISR);

        if enable {
            reg_w32(base, AR5K_PIMR, AR5K_INT_RXOK);
            reg_w32(base, AR5K_IER, AR5K_IER_ENABLE);
            let _ = reg_r32(base, AR5K_IER);
        } else {
            reg_w32(base, AR5K_PIMR, 0);
            let _ = reg_r32(base, AR5K_PIMR);
        }
    }
}

/// Acknowledge AR2413 interrupts safely on shared level-triggered PCI IRQ 11.
///
/// Checks `AR5K_INTPEND` first so non-owned interrupts return `0` without
/// disturbing other devices on IRQ 11, then reads `AR5K_RAC_PISR` and flushes
/// `AR5K_PISR` so the PCI INTA# line deasserts before PIC EOI.
#[allow(dead_code)]
pub(crate) unsafe fn ar2413_ack_irq(base: u32) -> u32 {
    unsafe {
        let pend = reg_r32(base, AR5K_INTPEND);
        if pend == AR5K_INT_NOCARD || (pend & 1) == 0 {
            return 0;
        }
        let pisr = reg_r32(base, AR5K_RAC_PISR);
        if pisr == AR5K_INT_NOCARD {
            return 0;
        }
        if pisr != 0 {
            reg_w32(base, AR5K_PISR, pisr);
            let _ = reg_r32(base, AR5K_PISR);
        }
        pisr
    }
}

// ---------------------------------------------------------------------
// Gate 4b.5 / 4b.6 — WPA2-PSK Connect + INT 60h Wi-Fi Data Path
// ---------------------------------------------------------------------

const TX_DESC_OFF: usize = 0x0300;
const TX_BUF_OFF: usize = 0x3800; // 2 KiB after the 8 RX buffers (0x0400..0x3600)
const AR5K_TSF_L32: usize = 0x804C;

/// Fixed 160-byte `#[repr(C)]` request/response struct shared with `WCONN.COM`
/// via JLM V86 API `AX=0x000C`.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct WifiConnectReq {
    pub ssid_len: u8,
    pub pass_len: u8,
    pub status: u8,
    pub channel: u8,
    pub bssid: [u8; 6],
    pub sta_mac: [u8; 6],
    pub ssid: [u8; 32],
    pub passphrase: [u8; 64],
    pub reserved: [u8; 48],
}

const _: () = {
    assert!(core::mem::size_of::<WifiConnectReq>() == 160);
};

/// Extra entropy for the SNonce from what this platform can offer: the
/// CMOS RTC time/date bytes, a free-running PIT channel-0 count, and EEPROM
/// words unique to this card (the MAC words and the header word).
///
/// Honest limit: none of this is a real randomness source. The RTC is
/// wall-clock, the PIT count is a fast timer whose value depends on when
/// the handshake happens to run, and the EEPROM words never change. Mixed
/// with the BIOS tick and the card TSF they still make SNonce repetition
/// across reconnects much less likely than the two timers alone — but PTK
/// uniqueness continues to rest mostly on the AP's ANonce, as it does in
/// any WPA2-PSK supplicant. RTC bytes may tear during a clock update;
/// for entropy that is harmless.
fn nonce_entropy(base: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    unsafe {
        // CMOS RTC: seconds/minutes/hours, day/month/year. Bit 7 of the
        // index port is the NMI gate; leave it clear (NMI enabled), the
        // state DOS runs in.
        let regs = [0x00u8, 0x02, 0x04, 0x07, 0x08, 0x09];
        for (i, reg) in regs.iter().enumerate() {
            outb(0x70, *reg);
            out[i] = inb(0x71);
        }
        // PIT channel 0 current count: latch, then read low/high.
        outb(0x43, 0x00);
        out[6] = inb(0x40);
        out[7] = inb(0x40);
    }
    // Per-card EEPROM words beside the MAC the caller already hashed in.
    if let Some(w) = unsafe { eeprom_read(base, 0x1D) } {
        out[8..10].copy_from_slice(&w.to_le_bytes());
    }
    if let Some(w) = unsafe { eeprom_read(base, 0x1E) } {
        out[10..12].copy_from_slice(&w.to_le_bytes());
    }
    if let Some(w) = unsafe { eeprom_read(base, 0x1F) } {
        out[12..14].copy_from_slice(&w.to_le_bytes());
    }
    if let Some(w) = unsafe { eeprom_read(base, AR5K_EEPROM_HDR) } {
        out[14..16].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn build_snonce(sta_mac: &[u8; 6], bssid: &[u8; 6], tsf: u32, extra: &[u8; 16]) -> [u8; 32] {
    let tick = bios_tick();
    let mut h1 = wifi_core::sha1::Sha1::new();
    h1.update(b"FERRIC-SNONCE-1");
    h1.update(sta_mac);
    h1.update(bssid);
    h1.update(&tick.to_le_bytes());
    h1.update(&tsf.to_le_bytes());
    h1.update(extra);
    let d1 = h1.finalize();

    let mut h2 = wifi_core::sha1::Sha1::new();
    h2.update(b"FERRIC-SNONCE-2");
    h2.update(&d1);
    h2.update(&tsf.to_le_bytes());
    h2.update(extra);
    let d2 = h2.finalize();

    let mut out = [0u8; 32];
    out[..20].copy_from_slice(&d1);
    out[20..32].copy_from_slice(&d2[..12]);
    out
}

unsafe fn find_target_ap(
    base: u32,
    dma_lin: u32,
    dma_phys: u32,
    target_ssid: &[u8],
) -> Option<([u8; 6], u8, bool)> {
    for ch in 1u8..=11u8 {
        let freq = 2407u16 + 5u16 * (ch as u16);
        unsafe { stop_rx_dma(base) };
        if ch > 1 {
            unsafe { ar2413_set_channel_2g(base, freq) };
        }
        unsafe {
            init_rx_ring(dma_lin, dma_phys);
            reg_w32(base, AR5K_RXDP, dma_phys);
            reg_w32(base, AR5K_CR, AR5K_CR_RXE);
            let _ = reg_r32(base, AR5K_CR);
        }

        let mut idx = 0usize;
        for _ in 0..160u32 {
            cardbus::delay_ms(1);
            // Whole-ring scan per tick, as in the data path: stopping at
            // the first unfinished descriptor strands completed ones
            // queued behind it.
            let start = idx;
            for step in 0..RX_DESC_COUNT {
                let cur = (start + step) % RX_DESC_COUNT;
                let doff = cur * RX_DESC_STRIDE;
                let stat1 = unsafe { dma_r32(dma_lin, doff + 20) };
                if (stat1 & AR5K_RX_STAT1_DONE) == 0 {
                    continue;
                }
                let stat0 = unsafe { dma_r32(dma_lin, doff + 16) };
                let dec = decode_5212_rx_status(stat0, stat1);
                let mut matched = None;
                if dec.ok && dec.len >= 36 {
                    let flen = (dec.len as usize).min(RX_BUF_SIZE);
                    let ptr = (dma_lin as usize + RX_BUF_BASE + cur * RX_BUF_SIZE) as *const u8;
                    let frame = unsafe { core::slice::from_raw_parts(ptr, flen) };
                    if let Some(b) = wifi_core::frame::parse_beacon(frame) {
                        if bytes_eq(b.ssid, target_ssid) {
                            matched = Some((b.bssid, b.channel.unwrap_or(ch), b.rsn.is_some()));
                        }
                    }
                }
                unsafe {
                    dma_w32(dma_lin, doff + 12, RX_BUF_SIZE as u32);
                    dma_w32(dma_lin, doff + 16, 0);
                    dma_w32(dma_lin, doff + 20, 0);
                }
                idx = (cur + 1) % RX_DESC_COUNT;
                if matched.is_some() {
                    return matched;
                }
            }
        }
    }
    None
}

fn hex_nibble(v: u32) -> u8 {
    let n = (v & 0xF) as u8;
    if n < 10 { b'0' + n } else { b'A' + (n - 10) }
}

fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// FFI entry for JLM V86 API `AX=0x000C`: scan for target SSID, run 802.11
/// Auth + Assoc + WPA2-PSK 4-way handshake (when `pass_len >= 8`), install
/// AR2413 hardware CCMP keys, and activate Wi-Fi as the `INT 60h` backend.
/// Returns `0` on success or non-zero status matching `req.status`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_wifi_connect(dma_seg: u32, req_lin: u32) -> u32 {
    if req_lin == 0 {
        return 1;
    }
    let req = unsafe { &mut *(req_lin as *mut WifiConnectReq) };
    let ssid_len = (req.ssid_len as usize).min(32);
    let pass_len = (req.pass_len as usize).min(64);
    if ssid_len == 0 {
        req.status = 3;
        return 3;
    }

    // If already connected (or CardBus already up from a prior session), cleanly
    // deauth, reset AR2413, and power-cycle the CardBus slot first so reconnecting
    // works reliably without rebooting.
    if ath_state().connected {
        unsafe {
            let _ = gate1_wifi_disconnect();
        }
    }

    // 1. Probe card & station MAC from EEPROM.
    let probe = probe_internal();
    if probe.present == 0 || probe.stage < 4 || probe.mac_ok == 0 {
        req.status = 1;
        return 1;
    }
    let base = probe.bar0_lin;
    let sta_mac = probe.mac;
    req.sta_mac = sta_mac;

    // 2. Lock caller's 16 KiB DMA block via VDMAD (PCI bus-master, no 64 KiB ISA limit).
    let dma_lin = dma_seg << 4;
    let Some(dma_phys) = (unsafe { vdmad_lock_dma_region_pci(dma_lin, WIFI_DMA_SIZE as u32) }) else {
        ath_state().last_fail = 2;
        req.status = 2;
        return 2;
    };

    // 3. Bring up AR2413 radio & locate target SSID on channels 1..11.
    ath_state().last_fail = 3;
    if !unsafe { ar2413_radio_init_11g(base, 2412) } {
        unsafe { vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32) };
        req.status = 2;
        return 2;
    }
    ath_state().last_fail = 0;
    let ssid_slice = &req.ssid[..ssid_len];
    let Some((bssid, ch, _ap_rsn)) =
        (unsafe { find_target_ap(base, dma_lin, dma_phys, ssid_slice) })
    else {
        unsafe {
            stop_rx_dma(base);
            vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32);
        }
        req.status = 3;
        return 3;
    };
    req.bssid = bssid;
    req.channel = ch;

    // 4. Full AR2413 radio init on AP channel, init TX QCU 0, clear keycache, set STA+BSSID, restart RX.
    let freq = 2407u16 + 5u16 * (ch as u16);
    unsafe {
        stop_rx_dma(base);
        ath_state().last_fail = 7;
        if !ar2413_radio_init_11g(base, freq) {
            vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32);
            req.status = 2;
            return 2;
        }
        ath_state().last_fail = 0;
        ar2413_init_tx_q0(base);
        ar2413_clear_keycache(base);
        ar2413_set_sta_bssid(base, &sta_mac, &bssid, 0);
        init_rx_ring(dma_lin, dma_phys);
        reg_w32(base, AR5K_RXDP, dma_phys);
        reg_w32(base, AR5K_CR, AR5K_CR_RXE);
        let _ = reg_r32(base, AR5K_CR);
    }

    let tx_desc_lin = dma_lin + TX_DESC_OFF as u32;
    let tx_desc_phys = dma_phys + TX_DESC_OFF as u32;
    let tx_buf_lin = dma_lin + TX_BUF_OFF as u32;
    let tx_buf_phys = dma_phys + TX_BUF_OFF as u32;
    let mut rx_idx = 0usize;
    let mut tx_seq: u16 = 1;
    let st = ath_state();

    // 5. Open-System Authentication (up to 3 attempts).
    let mut authed = false;
    for _attempt in 0..3 {
        let alen = wifi_core::frame::build_auth(&mut st.tx_frame, &sta_mac, &bssid, 1);
        tx_seq = (tx_seq + 1) & 0x0FFF;
        let _ = unsafe {
            ar2413_tx_frame_polled(
                base,
                tx_desc_lin,
                tx_desc_phys,
                tx_buf_lin,
                tx_buf_phys,
                &st.tx_frame[..alen],
                ATH5K_RATE_CODE_1M,
                None,
                false,
            )
        };
        for _ in 0..400u32 {
            cardbus::delay_ms(1);
            // Whole-ring scan per tick, as in the data path: stopping at
            // the first unfinished descriptor strands completed ones
            // queued behind it.
            let start = rx_idx;
            for step in 0..RX_DESC_COUNT {
                let cur = (start + step) % RX_DESC_COUNT;
                let doff = cur * RX_DESC_STRIDE;
                let stat1 = unsafe { dma_r32(dma_lin, doff + 20) };
                if (stat1 & AR5K_RX_STAT1_DONE) == 0 {
                    continue;
                }
                let stat0 = unsafe { dma_r32(dma_lin, doff + 16) };
                let dec = decode_5212_rx_status(stat0, stat1);
                if dec.ok && dec.len >= 30 {
                    let flen = (dec.len as usize).min(RX_BUF_SIZE);
                    let ptr =
                        (dma_lin as usize + RX_BUF_BASE + cur * RX_BUF_SIZE) as *const u8;
                    let frame = unsafe { core::slice::from_raw_parts(ptr, flen) };
                    if let Some((hdr, _)) = wifi_core::frame::parse_header(frame) {
                        if hdr.addr2 == bssid {
                            if let Some((aseq, astat)) = wifi_core::frame::parse_auth_resp(frame) {
                                if aseq == 2 && astat == 0 {
                                    authed = true;
                                }
                            }
                        }
                    }
                }
                unsafe {
                    dma_w32(dma_lin, doff + 12, RX_BUF_SIZE as u32);
                    dma_w32(dma_lin, doff + 16, 0);
                    dma_w32(dma_lin, doff + 20, 0);
                }
                rx_idx = (cur + 1) % RX_DESC_COUNT;
                if authed {
                    break;
                }
            }
            if authed {
                break;
            }
        }
        if authed {
            break;
        }
    }
    if !authed {
        unsafe {
            stop_rx_dma(base);
            vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32);
        }
        req.status = 4;
        return 4;
    }

    // 6. Association Request (with WPA2-PSK-CCMP RSN IE if passphrase provided).
    let use_wpa2 = pass_len >= 8;
    let mut assoc_aid: Option<u16> = None;
    for _attempt in 0..3 {
        let alen = wifi_core::frame::build_assoc_req_rsn(
            &mut st.tx_frame,
            &sta_mac,
            &bssid,
            ssid_slice,
            tx_seq,
            use_wpa2,
        );
        tx_seq = (tx_seq + 1) & 0x0FFF;
        let _ = unsafe {
            ar2413_tx_frame_polled(
                base,
                tx_desc_lin,
                tx_desc_phys,
                tx_buf_lin,
                tx_buf_phys,
                &st.tx_frame[..alen],
                ATH5K_RATE_CODE_1M,
                None,
                false,
            )
        };
        for _ in 0..400u32 {
            cardbus::delay_ms(1);
            // Whole-ring scan per tick, as in the data path: stopping at
            // the first unfinished descriptor strands completed ones
            // queued behind it.
            let start = rx_idx;
            for step in 0..RX_DESC_COUNT {
                let cur = (start + step) % RX_DESC_COUNT;
                let doff = cur * RX_DESC_STRIDE;
                let stat1 = unsafe { dma_r32(dma_lin, doff + 20) };
                if (stat1 & AR5K_RX_STAT1_DONE) == 0 {
                    continue;
                }
                let stat0 = unsafe { dma_r32(dma_lin, doff + 16) };
                let dec = decode_5212_rx_status(stat0, stat1);
                if dec.ok && dec.len >= 30 {
                    let flen = (dec.len as usize).min(RX_BUF_SIZE);
                    let ptr =
                        (dma_lin as usize + RX_BUF_BASE + cur * RX_BUF_SIZE) as *const u8;
                    let frame = unsafe { core::slice::from_raw_parts(ptr, flen) };
                    if let Some((hdr, _)) = wifi_core::frame::parse_header(frame) {
                        if hdr.addr2 == bssid {
                            if let Some((status, aid)) = wifi_core::frame::parse_assoc_resp(frame) {
                                if status == 0 {
                                    assoc_aid = Some(aid);
                                }
                            }
                        }
                    }
                }
                unsafe {
                    dma_w32(dma_lin, doff + 12, RX_BUF_SIZE as u32);
                    dma_w32(dma_lin, doff + 16, 0);
                    dma_w32(dma_lin, doff + 20, 0);
                }
                rx_idx = (cur + 1) % RX_DESC_COUNT;
                if assoc_aid.is_some() {
                    break;
                }
            }
            if assoc_aid.is_some() {
                break;
            }
        }
        if assoc_aid.is_some() {
            break;
        }
    }
    let Some(aid) = assoc_aid else {
        unsafe {
            stop_rx_dma(base);
            vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32);
        }
        req.status = 5;
        return 5;
    };
    unsafe { ar2413_set_sta_bssid(base, &sta_mac, &bssid, aid) };

    // 7. WPA2-PSK 4-Way Handshake + AR2413 Hardware AES-CCM Key Cache install.
    if use_wpa2 {
        let pmk = wifi_core::wpa::derive_pmk(&req.passphrase[..pass_len], ssid_slice);
        let tsf = unsafe { reg_r32(base, AR5K_TSF_L32) };
        let entropy = nonce_entropy(base);
        let snonce = build_snonce(&sta_mac, &bssid, tsf, &entropy);
        let mut hs = wifi_core::handshake::Wpa2Handshake::new(pmk, sta_mac, bssid, snonce);
        let mut keys_installed = false;

        for _ in 0..4000u32 {
            cardbus::delay_ms(1);
            // Whole-ring scan per tick, as in the data path: stopping at
            // the first unfinished descriptor strands completed ones
            // queued behind it — an M1 or M3 stuck there reads as a
            // handshake timeout.
            let start = rx_idx;
            for step in 0..RX_DESC_COUNT {
                let cur = (start + step) % RX_DESC_COUNT;
                let doff = cur * RX_DESC_STRIDE;
                let stat1 = unsafe { dma_r32(dma_lin, doff + 20) };
                if (stat1 & AR5K_RX_STAT1_DONE) == 0 {
                    continue;
                }
                let stat0 = unsafe { dma_r32(dma_lin, doff + 16) };
                let dec = decode_5212_rx_status(stat0, stat1);
                if dec.ok && dec.len >= 40 {
                    let flen = (dec.len as usize).min(RX_BUF_SIZE).saturating_sub(4);
                    let ptr =
                        (dma_lin as usize + RX_BUF_BASE + cur * RX_BUF_SIZE) as *const u8;
                    let frame = unsafe { core::slice::from_raw_parts(ptr, flen) };
                    if let Some(elen) =
                        wifi_core::frame::decap_80211_to_eth(&mut st.eth_rx, frame, &bssid)
                    {
                        if elen >= 18
                            && bytes_eq(&st.eth_rx[0..6], &sta_mac)
                            && st.eth_rx[12] == 0x88
                            && st.eth_rx[13] == 0x8E
                        {
                            match hs.process_eapol(&st.eth_rx[14..elen], &mut st.eapol_reply) {
                                wifi_core::handshake::HandshakeStep::SendM2(n) => {
                                    st.eth_tx[0..6].copy_from_slice(&bssid);
                                    st.eth_tx[6..12].copy_from_slice(&sta_mac);
                                    st.eth_tx[12] = 0x88;
                                    st.eth_tx[13] = 0x8E;
                                    st.eth_tx[14..14 + n].copy_from_slice(&st.eapol_reply[..n]);
                                    let dlen = wifi_core::frame::encap_eth_to_80211(
                                        &mut st.dot11_tx,
                                        &st.eth_tx[..14 + n],
                                        &bssid,
                                        &sta_mac,
                                        tx_seq,
                                        None,
                                    );
                                    tx_seq = (tx_seq + 1) & 0x0FFF;
                                    let mut m2_acked = false;
                                    for _ in 0..3 {
                                        if unsafe {
                                            ar2413_tx_frame_polled(
                                                base,
                                                tx_desc_lin,
                                                tx_desc_phys,
                                                tx_buf_lin,
                                                tx_buf_phys,
                                                &st.dot11_tx[..dlen],
                                                ATH5K_RATE_CODE_1M,
                                                None,
                                                false,
                                            )
                                        } {
                                            m2_acked = true;
                                            break;
                                        }
                                        cardbus::delay_ms(10);
                                    }
                                    if !m2_acked {
                                        unsafe {
                                            stop_rx_dma(base);
                                            vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32);
                                        }
                                        req.status = 6;
                                        return 6;
                                    }
                                }
                                wifi_core::handshake::HandshakeStep::SendM4 {
                                    len: n,
                                    tk,
                                    gtk,
                                    gtk_idx,
                                } => {
                                    st.eth_tx[0..6].copy_from_slice(&bssid);
                                    st.eth_tx[6..12].copy_from_slice(&sta_mac);
                                    st.eth_tx[12] = 0x88;
                                    st.eth_tx[13] = 0x8E;
                                    st.eth_tx[14..14 + n].copy_from_slice(&st.eapol_reply[..n]);
                                    let dlen = wifi_core::frame::encap_eth_to_80211(
                                        &mut st.dot11_tx,
                                        &st.eth_tx[..14 + n],
                                        &bssid,
                                        &sta_mac,
                                        tx_seq,
                                        None,
                                    );
                                    tx_seq = (tx_seq + 1) & 0x0FFF;
                                    let mut m4_acked = false;
                                    for _ in 0..3 {
                                        if unsafe {
                                            ar2413_tx_frame_polled(
                                                base,
                                                tx_desc_lin,
                                                tx_desc_phys,
                                                tx_buf_lin,
                                                tx_buf_phys,
                                                &st.dot11_tx[..dlen],
                                                ATH5K_RATE_CODE_1M,
                                                None,
                                                false,
                                            )
                                        } {
                                            m4_acked = true;
                                            break;
                                        }
                                        cardbus::delay_ms(10);
                                    }
                                    if !m4_acked {
                                        unsafe {
                                            stop_rx_dma(base);
                                            vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32);
                                        }
                                        req.status = 6;
                                        return 6;
                                    }
                                    cardbus::delay_ms(5);
                                    unsafe {
                                        // Slot 4: unicast PTK TK matching AP BSSID.
                                        ar2413_install_ccmp_key(base, 4, &tk, Some(&bssid));
                                        // Slot gtk_idx (1..=3): group GTK for broadcast/multicast RX.
                                        // Pass sta_mac with multicast bit set (byte 0 |= 0x01) so
                                        // encode_ccmp_key_words clears AR5K_KEYTABLE_VALID (key.c:107-108).
                                        if (1..=3).contains(&gtk_idx) {
                                            let mut gmac = sta_mac;
                                            gmac[0] |= 0x01;
                                            ar2413_install_ccmp_key(base, gtk_idx, &gtk, Some(&gmac));
                                        }
                                    }
                                    st.ptk_tk = tk;
                                    keys_installed = true;
                                }
                                wifi_core::handshake::HandshakeStep::Failed => {
                                    unsafe {
                                        stop_rx_dma(base);
                                        vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32);
                                    }
                                    req.status = 6;
                                    return 6;
                                }
                                // Rotation is a post-completion event; this
                                // loop leaves as soon as M4 installs the keys,
                                // so the arm only exists for exhaustiveness.
                                wifi_core::handshake::HandshakeStep::RotateGtk { .. } => {}
                                wifi_core::handshake::HandshakeStep::Ignore => {}
                            }
                        }
                    }
                }
                unsafe {
                    dma_w32(dma_lin, doff + 12, RX_BUF_SIZE as u32);
                    dma_w32(dma_lin, doff + 16, 0);
                    dma_w32(dma_lin, doff + 20, 0);
                }
                rx_idx = (cur + 1) % RX_DESC_COUNT;
                if keys_installed {
                    break;
                }
            }
            if keys_installed {
                break;
            }
        }
        if !keys_installed {
            unsafe {
                stop_rx_dma(base);
                vdmad_unlock_dma_region(dma_lin, WIFI_DMA_SIZE as u32);
            }
            req.status = 6;
            return 6;
        }

        // Keep the completed machine for the life of the session: when the
        // AP rotates the group key, wifi_poll_rx_eth feeds the rekey frame
        // to it, and only this machine — holding the PTK and the replay
        // floor — can authenticate and unwrap the new GTK.
        st.hs = Some(hs);
    }

    // 8. Save live Wi-Fi state and switch INT 60h packet driver backend to Wi-Fi.
    let st = ath_state();
    st.connected = true;
    st.dma_lin = dma_lin;
    st.dma_phys = dma_phys;
    st.rx_idx = 0;
    st.sta_mac = sta_mac;
    st.bssid = bssid;
    st.channel = ch;
    st.protected = use_wpa2;
    st.tx_seq = tx_seq;
    st.tx_pn = 1;
    st.tx_total = 0;
    st.tx_ok = 0;
    st.tx_fail_retry = 0;
    st.tx_fail_fifo = 0;
    st.tx_fail_filt = 0;
    st.tx_timeout = 0;
    st.tx_retries_sum = 0;
    st.tx_last_stat0 = 0;
    st.tx_last_stat1 = 0;
    st.rx_total = 0;
    st.rx_ok = 0;
    st.rx_err_key = 0;
    st.rx_err_dec = 0;
    st.rx_err_mic = 0;
    st.rx_err_crc = 0;
    st.rx_err_phy = 0;
    st.rx_err_other = 0;
    st.rx_decap_ok = 0;
    st.rx_decap_fail = 0;
    st.rx_ucast = 0;
    st.rx_ucast_arp = 0;
    st.rx_ucast_ip = 0;
    st.rx_nondata = 0;
    st.rx_nondata_last_fc = 0;
    st.rx_rearm = 0;
    st.rx_last_rssi = 0;
    st.rx_last_stat0 = 0;
    st.rx_last_stat1 = 0;
    st.wifi_ticks = 0;
    st.last_tx_len = 0;
    st.last_rxu_len = 0;

    unsafe {
        // Drop BEACON and MCAST from RX filter once connected so neighbor AP
        // beacons and LAN multicast (mDNS/SSDP/IPv6) do not flood the RX ring;
        // DOS mTCP only needs UCAST + BCAST (ARP, DHCP, ICMP, TCP, UDP).
        reg_w32(base, AR5K_MCAST_FILTER0_5211, 0);
        reg_w32(base, AR5K_MCAST_FILTER1_5211, 0);
        reg_w32(
            base,
            AR5K_RX_FILTER_5211,
            AR5K_RX_FILTER_UCAST | AR5K_RX_FILTER_BCAST,
        );
        // Re-arm a fresh RX ring starting at rx_idx=0 right after key install.
        stop_rx_dma(base);
        init_rx_ring(dma_lin, dma_phys);
        reg_w32(base, AR5K_RXDP, dma_phys);
        reg_w32(base, AR5K_CR, AR5K_CR_RXE);
        let _ = reg_r32(base, AR5K_CR);
        // Enable AR2413 hardware RXOK interrupt on shared PCI IRQ 11 (and unmask
        // slave PIC IRQ 11 + cascade IRQ 2) for sub-millisecond RX delivery;
        // INT 1Ch timer hook remains active as a safety-net poller.
        ar2413_set_irq_rx(base, true);
        let slave = inb(0xA1) & !0x08;
        outb(0xA1, slave);
        let master = inb(0x21) & !0x04;
        outb(0x21, master);
    }
    net_activate_wifi(sta_mac);

    req.status = 0;
    0
}

/// FFI entry for JLM V86 API `AX=0x000F` (`WPROBE /X`) and `DllMain(DLL_PROCESS_DETACH)`:
/// cleanly deauthenticate from the AP, halt AR2413 RX/TX DMA, disable IRQs, wipe
/// hardware & software keys, warm-reset AR2413, unlock VDMAD DMA memory, power down
/// the ToPIC95 CardBus 3.3 V rail (with 250 ms discharge), and hand `INT 60h` back
/// to Ethernet if initialized. Safe to call when already disconnected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_wifi_disconnect() -> u32 {
    let st = ath_state();
    let base = st.mapped_lin;
    if st.connected && base != 0 {
        unsafe {
            // 1. Best-effort 802.11 Deauthentication (reason 3 = STA leaving BSS)
            //    so the AP immediately drops the session before we cut radio power.
            if st.dma_lin != 0 && st.dma_phys != 0 {
                let dlen = wifi_core::frame::build_deauth(
                    &mut st.tx_frame,
                    &st.sta_mac,
                    &st.bssid,
                    st.tx_seq,
                    3,
                );
                st.tx_seq = (st.tx_seq + 1) & 0x0FFF;
                if dlen > 0 {
                    let _ = ar2413_tx_frame_polled(
                        base,
                        st.dma_lin + TX_DESC_OFF as u32,
                        st.dma_phys + TX_DESC_OFF as u32,
                        st.dma_lin + TX_BUF_OFF as u32,
                        st.dma_phys + TX_BUF_OFF as u32,
                        &st.tx_frame[..dlen],
                        ATH5K_RATE_CODE_1M,
                        None,
                        false,
                    );
                }
            }
            // 2. Halt RX DMA and TX QCU 0, mask IRQs, wipe HW key cache, warm-reset.
            stop_rx_dma(base);
            reg_w32(base, AR5K_QCU_TXD, 1);
            for _ in 0..20 {
                if (reg_r32(base, AR5K_QCU_TXE) & 1) == 0 {
                    break;
                }
                cardbus::delay_ms(1);
            }
            reg_w32(base, AR5K_QCU_TXD, 0);
            ar2413_set_irq_rx(base, false);
            ar2413_clear_keycache(base);
            let _ = nic_reset(
                base,
                AR5K_RESET_CTL_PCU | AR5K_RESET_CTL_BASEBAND | AR5K_RESET_CTL_PCI,
            );
            if st.dma_lin != 0 {
                vdmad_unlock_dma_region(st.dma_lin, WIFI_DMA_SIZE as u32);
            }
        }
    }

    st.connected = false;
    st.protected = false;
    st.ptk_tk = [0u8; 16];
    // Drop the stored handshake machine with the rest of the key material;
    // a reconnect builds a fresh one with a fresh SNonce.
    st.hs = None;
    st.dma_lin = 0;
    st.dma_phys = 0;

    unsafe {
        cardbus::power_down_slots();
    }
    net_deactivate_wifi();
    1
}

/// Heartbeat marker called from `gate1_net_poll_wifi()` on each INT 1Ch tick.
/// Also triggers a non-intrusive AR2413 noise-floor calibration every ~10 s.
pub(crate) fn wifi_timer_tick_mark() {
    unsafe {
        let st = ath_state();
        st.wifi_ticks = st.wifi_ticks.wrapping_add(1);
        if (st.wifi_ticks % 182) == 0 {
            if st.connected && st.mapped_lin != 0 {
                let agc = reg_r32(st.mapped_lin, AR5K_PHY_AGCCTL);
                if (agc & AR5K_PHY_AGCCTL_NF) == 0 {
                    reg_w32(st.mapped_lin, AR5K_PHY_AGCCTL, agc | AR5K_PHY_AGCCTL_NF);
                }
            }
        }
    }
}

/// Shared IRQ 11 front-half for active Wi-Fi: returns true if AR2413 asserted IRQ 11.
pub(crate) unsafe fn wifi_isr_ack() -> bool {
    let st = ath_state();
    if !st.connected || st.mapped_lin == 0 {
        return false;
    }
    unsafe { ar2413_ack_irq(st.mapped_lin) != 0 }
}

pub(crate) fn wifi_is_connected() -> bool {
    ath_state().connected
}

/// Disposition of a decapsulated Ethernet II frame received while connected.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RxEthAction {
    /// Deliver to the Crynwr packet-driver client.
    Deliver,
    /// Unicast EAPOL-Key PDU addressed to our station; feed to `Wpa2Handshake`.
    Eapol,
    /// Drop without delivering (plaintext on WPA2 session, self-broadcast echo,
    /// or non-unicast EAPOL).
    Drop,
}

/// Classify a decapsulated Ethernet II frame (`eth.len() >= 14`) on a live
/// Wi-Fi session:
/// - Unicast EtherType `0x888E` (EAPOL) addressed to `sta_mac` -> `Eapol`
///   (non-unicast EAPOL is invalid per IEEE 802.11i and dropped).
/// - On a WPA2-PSK session (`session_protected`), any non-EAPOL frame received
///   without the 802.11 Protected bit (`!frame_protected`) is dropped to block
///   unencrypted injection into mTCP.
/// - Broadcast/multicast frames whose source MAC equals `sta_mac` are AP
///   reflections of our own ToDS broadcasts (ARP/DHCP) and are dropped so the
///   DOS stack matches wired Ethernet semantics.
pub fn classify_rx_eth_frame(
    eth: &[u8],
    sta_mac: &[u8; 6],
    session_protected: bool,
    frame_protected: bool,
) -> RxEthAction {
    if eth.len() < 14 {
        return RxEthAction::Drop;
    }
    if session_protected && !frame_protected {
        return RxEthAction::Drop;
    }
    if eth[12] == 0x88 && eth[13] == 0x8E {
        if eth.len() >= 18 && bytes_eq(&eth[0..6], sta_mac) {
            return RxEthAction::Eapol;
        }
        return RxEthAction::Drop;
    }
    if (eth[0] & 0x01) != 0 && bytes_eq(&eth[6..12], sta_mac) {
        return RxEthAction::Drop;
    }
    RxEthAction::Deliver
}

/// Encapsulate an Ethernet II frame into `out_dot11` and, when `protected` is
/// true, software-encrypt it with IEEE 802.11i AES-128-CCM using `ptk_tk` and
/// the next monotonic `tx_pn`. Advances `tx_seq` (and `tx_pn` when protected).
/// Shared by `wifi_tx_eth` and post-connect EAPOL Group M2 replies so both
/// follow the exact same CCMP encryption and packet-number sequence.
pub fn encap_and_protect_eth(
    out_dot11: &mut [u8],
    eth: &[u8],
    bssid: &[u8; 6],
    sta_mac: &[u8; 6],
    protected: bool,
    ptk_tk: &[u8; 16],
    tx_seq: &mut u16,
    tx_pn: &mut u64,
) -> Option<usize> {
    let ccmp_pn = if protected {
        let pn = *tx_pn;
        *tx_pn = pn.wrapping_add(1);
        Some((pn, 0u8))
    } else {
        None
    };
    let mut dlen = wifi_core::frame::encap_eth_to_80211(
        out_dot11,
        eth,
        bssid,
        sta_mac,
        *tx_seq,
        ccmp_pn,
    );
    *tx_seq = (*tx_seq + 1) & 0x0FFF;
    if dlen == 0 {
        return None;
    }
    if protected {
        dlen = wifi_core::aes::ccmp_encrypt_frame(ptk_tk, out_dot11, dlen)?;
    }
    Some(dlen)
}

/// Feed a post-connect EAPOL PDU to the handshake machine kept from the
/// connect-time exchange. The only event that needs action is a group-key
/// rotation: install the new GTK through the same key-cache path connect
/// used, then answer with the group message 2 the machine built — the
/// rekey handshake has two messages, and an AP that never receives the
/// second retries the first and then commonly deauthenticates the station
/// for a failed group handshake. Everything else is ignored by the machine
/// itself, and its pairwise replies were a connect-time concern.
unsafe fn wifi_handle_eapol(eapol: &[u8]) {
    let st = ath_state();
    let base = st.mapped_lin;
    if base == 0 {
        return;
    }
    // The machine and the reply scratch are both fields of the state
    // struct, and process_eapol borrows them at the same time, so go
    // through a raw pointer to keep the two field borrows separate.
    let st_ptr: *mut Ath5kState = st;
    let step = unsafe {
        match (*st_ptr).hs.as_mut() {
            Some(hs) => hs.process_eapol(eapol, &mut (*st_ptr).eapol_reply),
            None => return,
        }
    };
    if let wifi_core::handshake::HandshakeStep::RotateGtk {
        gtk,
        gtk_idx,
        reply_len,
    } = step
    {
        if (1..=3).contains(&gtk_idx) {
            // Same group-key form as connect: station MAC with the multicast
            // bit set, so the key-cache entry matches group-addressed frames.
            let mut gmac = st.sta_mac;
            gmac[0] |= 0x01;
            unsafe { ar2413_install_ccmp_key(base, gtk_idx, &gtk, Some(&gmac)) };
        }
        // Per IEEE 802.11i §11.6.7.3, once the 4-way handshake has installed
        // the PTK, Group Key Handshake Message 2 MUST be CCMP-encrypted under
        // the PTK (APs drop unencrypted EAPOL frames once pairwise keys are
        // active). A failed send is not fatal: the AP retries message 1.
        if reply_len > 0 && 14 + reply_len <= st.eth_tx.len() {
            st.eth_tx[0..6].copy_from_slice(&st.bssid);
            st.eth_tx[6..12].copy_from_slice(&st.sta_mac);
            st.eth_tx[12] = 0x88;
            st.eth_tx[13] = 0x8E;
            st.eth_tx[14..14 + reply_len]
                .copy_from_slice(&st.eapol_reply[..reply_len]);
            let bssid = st.bssid;
            let sta_mac = st.sta_mac;
            let protected = st.protected;
            let ptk_tk = st.ptk_tk;
            let mut tx_seq = st.tx_seq;
            let mut tx_pn = st.tx_pn;
            let enc = encap_and_protect_eth(
                &mut st.dot11_tx,
                &st.eth_tx[..14 + reply_len],
                &bssid,
                &sta_mac,
                protected,
                &ptk_tk,
                &mut tx_seq,
                &mut tx_pn,
            );
            st.tx_seq = tx_seq;
            st.tx_pn = tx_pn;
            if let Some(dlen) = enc {
                for _ in 0..3 {
                    if unsafe {
                        ar2413_tx_frame_polled(
                            base,
                            st.dma_lin + TX_DESC_OFF as u32,
                            st.dma_phys + TX_DESC_OFF as u32,
                            st.dma_lin + TX_BUF_OFF as u32,
                            st.dma_phys + TX_BUF_OFF as u32,
                            &st.dot11_tx[..dlen],
                            ATH5K_RATE_CODE_1M,
                            None,
                            false,
                        )
                    } {
                        break;
                    }
                    cardbus::delay_ms(10);
                }
            }
        }
    }
}

/// Pop and decapsulate the next received 802.11 FromDS data frame into Ethernet II.
pub(crate) unsafe fn wifi_poll_rx_eth(out_eth: &mut [u8]) -> Option<usize> {
    let st = ath_state();
    if !st.connected || st.dma_lin == 0 {
        return None;
    }
    let mut found_any = false;
    // Recycling a descriptor advances rx_idx for the next poll.
    let start_idx = st.rx_idx;
    for step in 0..RX_DESC_COUNT {
        let idx = (start_idx + step) % RX_DESC_COUNT;
        let doff = idx * RX_DESC_STRIDE;
        let stat1 = unsafe { dma_r32(st.dma_lin, doff + 20) };
        if (stat1 & AR5K_RX_STAT1_DONE) == 0 {
            continue;
        }
        found_any = true;
        let stat0 = unsafe { dma_r32(st.dma_lin, doff + 16) };
        let dec = decode_5212_rx_status(stat0, stat1);
        st.rx_total = st.rx_total.wrapping_add(1);
        st.rx_last_stat0 = stat0;
        st.rx_last_stat1 = stat1;
        st.rx_last_rssi = dec.rssi;
        if dec.ok {
            st.rx_ok = st.rx_ok.wrapping_add(1);
        } else if (stat1 & 0x8000_0000) != 0 {
            st.rx_err_key = st.rx_err_key.wrapping_add(1);
        } else if (stat1 & 0x0000_0008) != 0 {
            st.rx_err_dec = st.rx_err_dec.wrapping_add(1);
        } else if (stat1 & 0x0000_0020) != 0 {
            st.rx_err_mic = st.rx_err_mic.wrapping_add(1);
        } else if (stat1 & 0x0000_0004) != 0 {
            st.rx_err_crc = st.rx_err_crc.wrapping_add(1);
        } else if (stat1 & 0x0000_0010) != 0 {
            st.rx_err_phy = st.rx_err_phy.wrapping_add(1);
        } else {
            st.rx_err_other = st.rx_err_other.wrapping_add(1);
        }
        let mut result = None;
        let ptr = (st.dma_lin as usize + RX_BUF_BASE + idx * RX_BUF_SIZE) as *const u8;
        let raw_len = (dec.len as usize).min(RX_BUF_SIZE);
        let raw_frame = unsafe { core::slice::from_raw_parts(ptr, raw_len) };
        let is_ucast_to_me = raw_len >= 10
            && bytes_eq(&raw_frame[4..10], &st.sta_mac)
            && raw_frame[0] != 0x48
            && raw_frame[0] != 0xC8;
        if is_ucast_to_me {
            st.rx_ucast = st.rx_ucast.wrapping_add(1);
        }
        if dec.ok && raw_len >= 2 && (raw_frame[0] & 0x0C) != 0x08 {
            st.rx_nondata = st.rx_nondata.wrapping_add(1);
            st.rx_nondata_last_fc = u16::from_le_bytes([raw_frame[0], raw_frame[1]]);
        }
        if dec.ok && dec.len >= 36 {
            let mut flen = raw_len.saturating_sub(4);
            let frame = &raw_frame[..flen];
            // If Protected bit is set in 802.11 FC[1] (0x40), trim 8-byte CCMP MIC trailer.
            let frame_protected = flen >= 12 && (frame[1] & 0x40) != 0;
            if frame_protected {
                flen -= 8;
            }
            if let Some(elen) =
                wifi_core::frame::decap_80211_to_eth(out_eth, &frame[..flen], &st.bssid)
            {
                match classify_rx_eth_frame(
                    &out_eth[..elen],
                    &st.sta_mac,
                    st.protected,
                    frame_protected,
                ) {
                    RxEthAction::Eapol => {
                        // EAPOL is supplicant traffic, never packet-driver
                        // traffic: post-connect it carries group-key rotations,
                        // which only the stored handshake machine can
                        // authenticate. Consume the frame here.
                        unsafe { wifi_handle_eapol(&out_eth[14..elen]) };
                        st.rx_decap_ok = st.rx_decap_ok.wrapping_add(1);
                    }
                    RxEthAction::Deliver => {
                        // Pad short Ethernet II frames (e.g. 42-byte ARP over 802.11) to
                        // the 60-byte Ethernet wire minimum expected by DOS packet drivers.
                        let mut final_len = elen;
                        if final_len < 60 && out_eth.len() >= 60 {
                            let mut p = final_len;
                            while p < 60 {
                                out_eth[p] = 0;
                                p += 1;
                            }
                            final_len = 60;
                        }
                        st.rx_decap_ok = st.rx_decap_ok.wrapping_add(1);
                        if is_ucast_to_me {
                            if out_eth[12] == 0x08 && out_eth[13] == 0x06 {
                                st.rx_ucast_arp = st.rx_ucast_arp.wrapping_add(1);
                            } else if out_eth[12] == 0x08 && out_eth[13] == 0x00 {
                                st.rx_ucast_ip = st.rx_ucast_ip.wrapping_add(1);
                            }
                            let cap = final_len.min(48);
                            st.last_rxu_len = final_len as u16;
                            st.last_rxu_rate = ((stat0 >> 15) & 0x1F) as u8;
                            st.last_rxu_eth[..cap].copy_from_slice(&out_eth[..cap]);
                        }
                        result = Some(final_len);
                    }
                    RxEthAction::Drop => {}
                }
            } else {
                st.rx_decap_fail = st.rx_decap_fail.wrapping_add(1);
            }
        }
        unsafe {
            dma_w32(st.dma_lin, doff + 12, RX_BUF_SIZE as u32);
            dma_w32(st.dma_lin, doff + 16, 0);
            dma_w32(st.dma_lin, doff + 20, 0);
        }
        st.rx_idx = (idx + 1) % RX_DESC_COUNT;
        if result.is_some() {
            return result;
        }
    }
    if !found_any {
        // If RX DMA halted (e.g. RXORN/RXEOL), re-arm RXDP + RXE cleanly.
        unsafe {
            if (reg_r32(st.mapped_lin, AR5K_CR) & AR5K_CR_RXE) == 0 {
                st.rx_rearm = st.rx_rearm.wrapping_add(1);
                let cur_phys = st.dma_phys + (st.rx_idx * RX_DESC_STRIDE) as u32;
                reg_w32(st.mapped_lin, AR5K_RXDP, cur_phys);
                reg_w32(st.mapped_lin, AR5K_CR, AR5K_CR_RXE);
                let _ = reg_r32(st.mapped_lin, AR5K_CR);
            }
        }
    }
    None
}

/// Encapsulate an Ethernet II frame from DOS linear memory and transmit over AR2413.
pub(crate) unsafe fn wifi_tx_eth(src_lin: u32, len: usize) -> bool {
    let st = ath_state();
    if !st.connected || st.mapped_lin == 0 || st.dma_lin == 0 || len < 14 || len > 1514 {
        return false;
    }
    let eth = unsafe { core::slice::from_raw_parts(src_lin as *const u8, len) };
    let tcap = len.min(48);
    st.last_tx_len = len as u16;
    st.last_tx_eth[..tcap].copy_from_slice(&eth[..tcap]);
    let bssid = st.bssid;
    let sta_mac = st.sta_mac;
    let protected = st.protected;
    let ptk_tk = st.ptk_tk;
    let mut tx_seq = st.tx_seq;
    let mut tx_pn = st.tx_pn;
    // Software IEEE 802.11i AES-128-CCM TX encryption (RX stays on AR2413 HW AES-CCM).
    // Passes key_idx=None so AR2413 transmits the pre-encrypted MPDU + MIC verbatim.
    let Some(dlen) = encap_and_protect_eth(
        &mut st.dot11_tx,
        eth,
        &bssid,
        &sta_mac,
        protected,
        &ptk_tk,
        &mut tx_seq,
        &mut tx_pn,
    ) else {
        st.tx_seq = tx_seq;
        st.tx_pn = tx_pn;
        return false;
    };
    st.tx_seq = tx_seq;
    st.tx_pn = tx_pn;
    unsafe {
        ar2413_tx_frame_polled(
            st.mapped_lin,
            st.dma_lin + TX_DESC_OFF as u32,
            st.dma_phys + TX_DESC_OFF as u32,
            st.dma_lin + TX_BUF_OFF as u32,
            st.dma_phys + TX_BUF_OFF as u32,
            &st.dot11_tx[..dlen],
            ATH5K_RATE_CODE_36M,
            None,
            false,
        )
    }
}

struct DiagBuf<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> DiagBuf<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn push_byte(&mut self, b: u8) {
        if self.pos < self.buf.len() {
            self.buf[self.pos] = b;
            self.pos += 1;
        }
    }
    fn push_str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.push_byte(b);
        }
    }
    fn push_crlf(&mut self) {
        self.push_str("\r\n");
    }
    fn push_hex32(&mut self, v: u32) {
        let mut s = 28;
        loop {
            self.push_byte(hex_nibble(v >> s));
            if s == 0 {
                break;
            }
            s -= 4;
        }
    }
    fn push_hex16(&mut self, v: u16) {
        self.push_byte(hex_nibble((v >> 12) as u32));
        self.push_byte(hex_nibble((v >> 8) as u32));
        self.push_byte(hex_nibble((v >> 4) as u32));
        self.push_byte(hex_nibble(v as u32));
    }
    fn push_u32(&mut self, mut v: u32) {
        let mut tmp = [0u8; 10];
        let mut n = 0usize;
        if v == 0 {
            self.push_byte(b'0');
            return;
        }
        while v > 0 && n < 10 {
            tmp[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.push_byte(tmp[n]);
        }
    }
    fn push_i32(&mut self, v: i32) {
        if v < 0 {
            self.push_byte(b'-');
            self.push_u32(v.wrapping_neg() as u32);
        } else {
            self.push_u32(v as u32);
        }
    }
    fn push_kv_u32(&mut self, k: &str, v: u32) {
        self.push_str(k);
        self.push_byte(b'=');
        self.push_u32(v);
        self.push_byte(b' ');
    }
    fn push_kv_hex32(&mut self, k: &str, v: u32) {
        self.push_str(k);
        self.push_byte(b'=');
        self.push_hex32(v);
        self.push_byte(b' ');
    }
}

/// FFI entry for JLM V86 API `AX=0x000D` (`WPROBE /D`): format a live, non-destructive
/// ASCII diagnostic report of Wi-Fi TX/RX counters, AR2413 MAC/PHY register readback,
/// and 11g EEPROM calibration fields into `dst_lin[..cap]`. Returns bytes written.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_wifi_diag(dst_lin: u32, cap: u32) -> u32 {
    if dst_lin == 0 || cap == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(dst_lin as *mut u8, cap as usize) };
    let mut w = DiagBuf::new(slice);
    let st = ath_state();

    w.push_str("WDIAG: ");
    w.push_kv_u32("conn", u32::from(st.connected));
    w.push_kv_u32("ch", st.channel as u32);
    w.push_kv_u32("prot", u32::from(st.protected));
    w.push_kv_u32("seq", st.tx_seq as u32);
    w.push_kv_u32("pn", st.tx_pn as u32);
    w.push_kv_u32("ticks", st.wifi_ticks);
    w.push_kv_u32("fail", st.last_fail as u32);
    w.push_crlf();

    w.push_str("TX: ");
    w.push_kv_u32("tot", st.tx_total);
    w.push_kv_u32("ok", st.tx_ok);
    w.push_kv_u32("xret", st.tx_fail_retry);
    w.push_kv_u32("fifo", st.tx_fail_fifo);
    w.push_kv_u32("filt", st.tx_fail_filt);
    w.push_kv_u32("tmo", st.tx_timeout);
    w.push_kv_u32("rets", st.tx_retries_sum);
    w.push_kv_hex32("s0", st.tx_last_stat0);
    w.push_kv_hex32("s1", st.tx_last_stat1);
    w.push_crlf();

    w.push_str("RX: ");
    w.push_kv_u32("tot", st.rx_total);
    w.push_kv_u32("ok", st.rx_ok);
    w.push_kv_u32("dec_ok", st.rx_decap_ok);
    w.push_kv_u32("dec_f", st.rx_decap_fail);
    w.push_kv_u32("rearm", st.rx_rearm);
    w.push_kv_u32("rssi", st.rx_last_rssi as u32);
    w.push_crlf();

    let (nrx, ndrp, ntx, nerr) = crate::net_diag_counts();
    w.push_str("UCAST: ");
    w.push_kv_u32("tot", st.rx_ucast);
    w.push_kv_u32("arp", st.rx_ucast_arp);
    w.push_kv_u32("ip", st.rx_ucast_ip);
    w.push_kv_u32("nd", st.rx_nondata);
    w.push_str("ndfc=");
    w.push_hex16(st.rx_nondata_last_fc);
    w.push_str(" NET: ");
    w.push_kv_u32("rx", nrx);
    w.push_kv_u32("drp", ndrp);
    w.push_kv_u32("tx", ntx);
    w.push_kv_u32("err", nerr);
    w.push_crlf();

    w.push_str("LTX(");
    w.push_u32(st.last_tx_len as u32);
    w.push_str("): ");
    let tlen = (st.last_tx_len as usize).min(48);
    for i in 0..tlen {
        let b = st.last_tx_eth[i];
        w.push_byte(hex_nibble((b >> 4) as u32));
        w.push_byte(hex_nibble(b as u32));
    }
    w.push_crlf();

    w.push_str("LRXU(");
    w.push_u32(st.last_rxu_len as u32);
    w.push_str(",r=");
    w.push_byte(hex_nibble((st.last_rxu_rate >> 4) as u32));
    w.push_byte(hex_nibble(st.last_rxu_rate as u32));
    w.push_str("): ");
    let rlen = (st.last_rxu_len as usize).min(48);
    for i in 0..rlen {
        let b = st.last_rxu_eth[i];
        w.push_byte(hex_nibble((b >> 4) as u32));
        w.push_byte(hex_nibble(b as u32));
    }
    w.push_crlf();

    w.push_str("RXERR: ");
    w.push_kv_u32("key", st.rx_err_key);
    w.push_kv_u32("dec", st.rx_err_dec);
    w.push_kv_u32("mic", st.rx_err_mic);
    w.push_kv_u32("crc", st.rx_err_crc);
    w.push_kv_u32("phy", st.rx_err_phy);
    w.push_kv_u32("oth", st.rx_err_other);
    w.push_kv_hex32("s0", st.rx_last_stat0);
    w.push_kv_hex32("s1", st.rx_last_stat1);
    w.push_crlf();

    // Live ToPIC100 Slot 0 + child PCI config + first-SREV abort snapshot.
    let cb = unsafe { cardbus::read_slot_diag(0) };
    w.push_str("CB: ");
    w.push_kv_hex32("st", cb.sock_state);
    w.push_kv_hex32("ctl", cb.sock_ctrl);
    w.push_kv_hex32("bcs", cb.br_cmd_stat);
    w.push_str("ss=");
    w.push_hex16(cb.br_sec_stat);
    w.push_str(" bc=");
    w.push_hex16(cb.br_bctrl);
    w.push_byte(b' ');
    w.push_kv_hex32("rcr", cb.topic_rcr);
    w.push_crlf();

    w.push_str("CB2: ");
    w.push_kv_hex32("m0b", cb.br_mem0_base);
    w.push_kv_hex32("m0l", cb.br_mem0_lim);
    w.push_kv_hex32("ccs", cb.ch_cmd_stat);
    w.push_kv_hex32("cbar", cb.ch_bar0);
    w.push_str("pm=");
    w.push_hex16(cb.ch_pmcsr);
    w.push_crlf();

    w.push_str("CBSR: ");
    w.push_kv_hex32("srev0", st.srev_first);
    w.push_str("p0=");
    w.push_hex16(st.pci_pri_before);
    w.push_str(" s0=");
    w.push_hex16(st.pci_sec_before);
    w.push_str(" p1=");
    w.push_hex16(st.pci_pri_after);
    w.push_str(" s1=");
    w.push_hex16(st.pci_sec_after);
    w.push_str(" heal=");
    w.push_u32(st.self_healed as u32);
    w.push_crlf();

    let base = st.mapped_lin;
    if base == 0 || !cardbus::any_slot_powered() {
        w.push_str("REGS: powered_down\r\n");
        return w.pos as u32;
    }

    unsafe {
        w.push_str("MAC: ");
        w.push_kv_hex32("CR", reg_r32(base, AR5K_CR));
        w.push_kv_hex32("TXCFG", reg_r32(base, AR5K_TXCFG));
        w.push_kv_hex32("RXCFG", reg_r32(base, AR5K_RXCFG));
        w.push_kv_hex32("STA1", reg_r32(base, AR5K_STA_ID1));
        w.push_kv_hex32("DANT", reg_r32(base, AR5K_DEFAULT_ANTENNA));
        w.push_kv_hex32("RFILT", reg_r32(base, AR5K_RX_FILTER_5211));
        w.push_kv_hex32("DIAG", reg_r32(base, AR5K_DIAG_SW_5211));
        w.push_crlf();

        let k4 = AR5K_KEYTABLE_0 + 4 * 32;
        w.push_str("KEY4: ");
        w.push_kv_hex32("T", reg_r32(base, k4 + 20));
        w.push_kv_hex32("M0", reg_r32(base, k4 + 24));
        w.push_kv_hex32("M1", reg_r32(base, k4 + 28));
        w.push_kv_hex32("BSS0", reg_r32(base, AR5K_BSS_ID0));
        w.push_kv_hex32("BSS1", reg_r32(base, AR5K_BSS_ID1));
        w.push_crlf();

        w.push_str("ANT: ");
        w.push_kv_hex32("CTL", reg_r32(base, AR5K_PHY_ANT_CTL));
        w.push_kv_hex32("SW0", reg_r32(base, AR5K_PHY_ANT_SWITCH_TABLE_0));
        w.push_kv_hex32("SW1", reg_r32(base, AR5K_PHY_ANT_SWITCH_TABLE_1));
        w.push_kv_hex32("FDIV", reg_r32(base, AR5K_PHY_FAST_ANT_DIV));
        w.push_kv_hex32("AGC", reg_r32(base, AR5K_PHY_AGCCTL));
        w.push_crlf();

        w.push_str("PHY: ");
        w.push_kv_hex32("SET", reg_r32(base, AR5K_PHY_SETTLING));
        w.push_kv_hex32("GAIN", reg_r32(base, AR5K_PHY_GAIN));
        w.push_kv_hex32("DES", reg_r32(base, AR5K_PHY_DESIRED_SIZE));
        w.push_kv_hex32("G2G", reg_r32(base, AR5K_PHY_GAIN_2GHZ));
        w.push_kv_hex32("CTL3", reg_r32(base, AR5K_PHY_RF_CTL3));
        w.push_kv_hex32("CTL4", reg_r32(base, AR5K_PHY_RF_CTL4));
        w.push_crlf();

        w.push_str("PHY2: ");
        w.push_kv_hex32("NF", reg_r32(base, AR5K_PHY_NF));
        w.push_kv_hex32("NFTHR", reg_r32(base, AR5K_PHY_NFTHRES));
        w.push_kv_hex32("IQ", reg_r32(base, AR5K_PHY_IQ));
        w.push_kv_hex32("PWADJ", reg_r32(base, AR5K_PHY_TX_PWR_ADJ));
        w.push_kv_hex32("RG1", reg_r32(base, 0xA258));
        w.push_kv_hex32("RG5", reg_r32(base, 0xA26C));
        w.push_crlf();

        w.push_str("PWR: ");
        w.push_kv_hex32("R1", reg_r32(base, 0x9934));
        w.push_kv_hex32("R2", reg_r32(base, 0x9938));
        w.push_kv_hex32("RMAX", reg_r32(base, 0x993C));
        w.push_kv_hex32("R3", reg_r32(base, 0xA234));
        w.push_kv_hex32("R4", reg_r32(base, 0xA238));
        w.push_crlf();

        w.push_str("PDADC0..3: ");
        w.push_kv_hex32("P0", reg_r32(base, 0xA280));
        w.push_kv_hex32("P1", reg_r32(base, 0xA284));
        w.push_kv_hex32("P2", reg_r32(base, 0xA288));
        w.push_kv_hex32("P3", reg_r32(base, 0xA28C));
        w.push_crlf();

        if let Some(ee_ver) = eeprom_read(base, 0x00C1) {
            let off = if ee_ver >= 0x3003 { 0x010Du32 } else { 0x00DAu32 };
            let mut ew = [0u16; 5];
            let mut ok = true;
            for i in 0..5 {
                if let Some(v) = eeprom_read(base, off + i as u32) {
                    ew[i] = v;
                } else {
                    ok = false;
                }
            }
            let mut em = [0u16; 12];
            for i in 0..12 {
                if let Some(v) = eeprom_read(base, off + 5 + i as u32) {
                    em[i] = v;
                } else {
                    ok = false;
                }
            }
            if ok {
                let ants = decode_eeprom_ants(ew);
                let modes = decode_eeprom_modes_11g(ee_ver, &em);
                let misc0 = eeprom_read(base, 0x00C4).unwrap_or(0);
                w.push_str("EE11G: ver=");
                w.push_hex16(ee_ver);
                w.push_str(" misc0=");
                w.push_hex16(misc0);
                w.push_str(" idle=");
                w.push_u32(ants.ant_idle);
                w.push_str(" swA=");
                w.push_hex32(ants.swtable_a);
                w.push_str(" swB=");
                w.push_hex32(ants.swtable_b);
                w.push_crlf();

                w.push_str("EE11G2: ");
                w.push_kv_u32("ob", modes.ob_1 as u32);
                w.push_kv_u32("db", modes.db_1 as u32);
                w.push_kv_u32("adc", modes.adc_desired_size as u32);
                w.push_kv_u32("pga", modes.pga_desired_size as u32);
                w.push_str("nf=");
                w.push_i32(modes.noise_floor_thr as i32);
                w.push_byte(b' ');
                w.push_kv_u32("xpaE", modes.tx_frm2xpa_enable as u32);
                w.push_kv_u32("xpaD", modes.tx_end2xpa_disable as u32);
                w.push_kv_u32("xlna", modes.tx_end2xlna_enable as u32);
                w.push_kv_u32("t62", modes.thr_62 as u32);
                w.push_kv_u32("mrg", modes.margin_tx_rx as u32);
                w.push_crlf();
            }
        }
    }

    w.pos as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_support_matches_ath5k_attach_rules() {
        // AR2413 (0x78), EEPROM 5.1, AES_DIS clear -> hardware AES-CCM supported.
        assert!(decode_aes_support(AR5K_SREV_AR2413, 0x5001, 0x0000));
        // AR2413 (0x78), EEPROM 5.1, AES_DIS set (bit 1) -> disabled.
        assert!(!decode_aes_support(AR5K_SREV_AR2413, 0x5001, 0x0002));
        // AR2413 (0x78), pre-5.0 EEPROM -> enabled even if misc5 bit 1 were set.
        assert!(decode_aes_support(AR5K_SREV_AR2413, 0x4009, 0x0002));
        // Pre-AR5212v4 silicon -> no hardware AES-CCM.
        assert!(!decode_aes_support(0x50, 0x5001, 0x0000));
    }

    #[test]
    fn eeprom_mac_decode_matches_ath5k_word_order() {
        // D-Link OUI 00:13:46:AA:BB:CC stored in 0x1F=0x0013, 0x1E=0x46AA, 0x1D=0xBBCC.
        assert_eq!(
            decode_eeprom_mac(0x0013, 0x46AA, 0xBBCC),
            Some([0x00, 0x13, 0x46, 0xAA, 0xBB, 0xCC])
        );
        assert_eq!(decode_eeprom_mac(0x0000, 0x0000, 0x0000), None);
        assert_eq!(decode_eeprom_mac(0xFFFF, 0xFFFF, 0xFFFF), None);
    }

    #[test]
    fn wifi_probe_info_size_matches_asm_contract() {
        assert_eq!(core::mem::size_of::<WifiProbeInfo>(), 44);
        assert_eq!(core::mem::size_of::<ScanEntry>(), 44);
        assert_eq!(core::mem::size_of::<WifiScanResult>(), 720);
    }

    #[test]
    fn rf5112_channel_regs_match_ath5k_formula() {
        // Ch 1 (2412 MHz): raw = (2*(2412-672)-3040)/10 = 44 = 0x2C, <<2 = 0xB0, bitswap8(0xB0)=0x0D
        // data = (0x0D << 4) | 0x1001 = 0x10D1 -> lo=0xD1, hi=0x10.
        assert_eq!(rf5112_channel_regs(2412), Some((0xD1, 0x10)));
        // Ch 6 (2437 MHz): raw = 49 = 0x31, <<2 = 0xC4, bitswap8(0xC4)=0x23 -> data=0x1231 -> (0x31, 0x12).
        assert_eq!(rf5112_channel_regs(2437), Some((0x31, 0x12)));
        // Ch 11 (2462 MHz): raw = 54 = 0x36, <<2 = 0xD8, bitswap8(0xD8)=0x1B -> data=0x11B1 -> (0xB1, 0x11).
        assert_eq!(rf5112_channel_regs(2462), Some((0xB1, 0x11)));
        assert_eq!(rf5112_channel_regs(2400), None);
    }

    #[test]
    fn rx_desc_decode_extracts_len_rssi_and_flags() {
        let stat0 = 128u32 | (42u32 << 20);
        let stat1 = AR5K_RX_STAT1_DONE | AR5K_RX_STAT1_OK;
        let d = decode_5212_rx_status(stat0, stat1);
        assert!(d.done);
        assert!(d.ok);
        assert_eq!(d.len, 128);
        assert_eq!(d.rssi, 42);
    }

    #[test]
    fn tx_ctl_4w_open_mgmt_frame() {
        let d = build_5212_tx_ctl(64, ATH5K_RATE_CODE_1M, None, false);
        assert_eq!(d.ctl0 & AR5K_4W_TX_DESC_CTL0_FRAME_LEN, 64 + 4);
        assert_eq!(
            (d.ctl0 & AR5K_4W_TX_DESC_CTL0_XMIT_POWER) >> AR5K_4W_TX_DESC_CTL0_XMIT_POWER_S,
            60
        );
        assert_ne!(d.ctl0 & AR5K_4W_TX_DESC_CTL0_CLRDMASK, 0);
        assert_eq!(d.ctl0 & AR5K_4W_TX_DESC_CTL0_ENCRYPT_KEY_VALID, 0);
        assert_eq!(d.ctl1 & AR5K_4W_TX_DESC_CTL1_BUF_LEN, 64);
        assert_eq!(d.ctl1 & AR5K_4W_TX_DESC_CTL1_ENCRYPT_KEY_IDX, 0);
        assert_eq!(d.ctl1 & AR5K_4W_TX_DESC_CTL1_NOACK, 0);
        assert_eq!(
            (d.ctl2 & AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0) >> AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0_S,
            10
        );
        assert_eq!(d.ctl3 & AR5K_4W_TX_DESC_CTL3_XMIT_RATE0, 0x1B);

        // Broadcast / no-ack frame sets NOACK bit in ctl1.
        let b = build_5212_tx_ctl(48, ATH5K_RATE_CODE_1M, None, true);
        assert_ne!(b.ctl1 & AR5K_4W_TX_DESC_CTL1_NOACK, 0);
        assert_eq!(b.ctl0 & AR5K_4W_TX_DESC_CTL0_FRAME_LEN, 52);
    }

    #[test]
    fn tx_ctl_4w_ccmp_data_frame() {
        let d = build_5212_tx_ctl(100, ATH5K_RATE_CODE_6M, Some(4), false);
        // FRAME_LEN includes +8 CCMP MIC +4 FCS = 112; BUF_LEN includes +8 MIC = 108.
        assert_eq!(d.ctl0 & AR5K_4W_TX_DESC_CTL0_FRAME_LEN, 100 + 8 + 4);
        assert_ne!(d.ctl0 & AR5K_4W_TX_DESC_CTL0_ENCRYPT_KEY_VALID, 0);
        assert_eq!(d.ctl1 & AR5K_4W_TX_DESC_CTL1_BUF_LEN, 100 + 8);
        assert_eq!(
            (d.ctl1 & AR5K_4W_TX_DESC_CTL1_ENCRYPT_KEY_IDX)
                >> AR5K_4W_TX_DESC_CTL1_ENCRYPT_KEY_IDX_S,
            4
        );
        assert_eq!(d.ctl1 & AR5K_4W_TX_DESC_CTL1_NOACK, 0);
        assert_eq!(d.ctl2, (10u32 << 16) | (7u32 << 20));
        assert_eq!(d.ctl3, 0x0B | ((ATH5K_RATE_CODE_1M as u32) << 5));

        // Zero rate code falls back to 1 Mbps CCK (0x1B) to prevent HW noise lockup.
        let safe = build_5212_tx_ctl(60, 0, None, false);
        assert_eq!(safe.ctl3, ATH5K_RATE_CODE_1M as u32);
    }

    #[test]
    fn ccmp_key_words_pack_key_and_unicast_mac() {
        let key: [u8; 16] = [
            0x01, 0x02, 0x03, 0x04, // key0 = 0x04030201
            0x05, 0x06, // key1 = 0x00000605
            0x07, 0x08, 0x09, 0x0A, // key2 = 0x0A090807
            0x0B, 0x0C, // key3 = 0x00000C0B
            0x0D, 0x0E, 0x0F, 0x10, // key4 = 0x100F0E0D
        ];
        // Unicast MAC with odd byte[4] (0x55) to verify bit 0 of macHi shifts into bit 31 of macLo.
        let mac: [u8; 6] = [0x00, 0x11, 0x22, 0x33, 0x55, 0x66];
        let w = encode_ccmp_key_words(&key, Some(&mac));

        assert_eq!(w[0], 0x0403_0201);
        assert_eq!(w[1], 0x0000_0605);
        assert_eq!(w[2], 0x0A09_0807);
        assert_eq!(w[3], 0x0000_0C0B);
        assert_eq!(w[4], 0x100F_0E0D);
        assert_eq!(w[5], AR5K_KEYTABLE_TYPE_CCM);
        // raw_lo = 0x33221100, raw_hi = 0x6655 (bit 0 = 1)
        // mac_lo = (0x33221100 >> 1) | (1 << 31) = 0x9991_0880
        // mac_hi = (0x6655 >> 1) | 0x8000 = 0x332A | 0x8000 = 0xB32A
        assert_eq!(w[6], 0x9991_0880);
        assert_eq!(w[7], 0x0000_B32A);
    }

    #[test]
    fn ccmp_key_words_group_and_multicast_mac() {
        let key = [0xAAu8; 16];
        // Group key (mac = None): MAC0 = 0, MAC1 = AR5K_KEYTABLE_VALID (0x8000).
        let grp = encode_ccmp_key_words(&key, None);
        assert_eq!(grp[5], AR5K_KEYTABLE_TYPE_CCM);
        assert_eq!(grp[6], 0);
        assert_eq!(grp[7], AR5K_KEYTABLE_VALID);

        // Multicast MAC (bit 0 of byte 0 set): VALID bit cleared per ath/key.c:107-108.
        let mcast: [u8; 6] = [0x01, 0x00, 0x5E, 0x00, 0x00, 0xFB];
        let mc = encode_ccmp_key_words(&key, Some(&mcast));
        assert_eq!(mc[7] & AR5K_KEYTABLE_VALID, 0);
        // raw_lo = 0x005E0001, raw_hi = 0xFB00 (bit 0 = 0)
        // mac_lo = 0x002F0000, mac_hi = 0x7D80
        assert_eq!(mc[6], 0x002F_0000);
        assert_eq!(mc[7], 0x0000_7D80);
    }

    #[test]
    fn eeprom_ants_decode_matches_ath5k_bit_packing() {
        // Pack known 6-bit values a0..a10 = 1..=11, switch_settling=0x2D, atn_tx_rx=0x15.
        let sw_set: u16 = 0x2D;
        let atn: u16 = 0x15;
        let a: [u16; 11] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        let w0 = (sw_set << 8) | (atn << 2) | (a[0] >> 4);
        let w1 = ((a[0] & 0xF) << 12) | (a[1] << 6) | a[2];
        let w2 = (a[3] << 10) | (a[4] << 4) | (a[5] >> 2);
        let w3 = ((a[5] & 0x3) << 14) | (a[6] << 8) | (a[7] << 2) | (a[8] >> 4);
        let w4 = ((a[8] & 0xF) << 12) | (a[9] << 6) | a[10];
        let dec = decode_eeprom_ants([w0, w1, w2, w3, w4]);
        assert_eq!(dec.switch_settling, 0x2D);
        assert_eq!(dec.atn_tx_rx, 0x15);
        assert_eq!(dec.ant_idle, 1);
        assert_eq!(
            dec.swtable_a,
            2 | (3 << 6) | (4 << 12) | (5 << 18) | (6 << 24)
        );
        assert_eq!(
            dec.swtable_b,
            7 | (8 << 6) | (9 << 12) | (10 << 18) | (11 << 24)
        );
    }

    #[test]
    fn eeprom_modes_11g_decode_matches_ath5k_v5_layout() {
        // Synthetic EEPROM v5.1 11g mode words (m[0..12]).
        let mut m = [0u16; 12];
        m[0] = (0xE0u16 << 8) | (3u16 << 4) | 2u16; // adc=-32 (0xE0), ob_1=3, db_1=2
        m[1] = (0x0Au16 << 8) | 0x1Cu16; // tx_end2xlna=10, thr_62=28
        m[2] = (0x05u16 << 8) | 0x06u16; // tx_end2xpa_dis=5, tx_frm2xpa_en=6
        m[3] = (0xD6u16 << 8) | 0xCAu16; // pga=-42 (0xD6), nf_thr=-54 (0xCA)
        m[4] = 0; // xlna/x_gain/xpd
        m[5] = 0; // v3.3+ false_detect/ob_0/db_0/i_gain_lo
        m[6] = 0x14u16 << 3; // v3.4+ cck_ofdm_power_delta = +20
        m[7] = 0xFFFF; // v4.0+ pwr_cal_g[0..1]
        m[8] = 0; // turbo/xr
        m[9] = (0x12u16 << 8) | 0xFF; // v4.1+ margin_tx_rx = 0x12
        m[10] = (0x1Au16 << 5) | 0x0Du16; // i_cal = 0x1A, q_cal = 0x0D
        m[11] = 0x00FBu16; // v4.2+ cck_ofdm_gain_delta = -5 (0xFB)

        let d = decode_eeprom_modes_11g(0x5001, &m);
        assert_eq!(d.adc_desired_size, 0xE0);
        assert_eq!(d.ob_1, 3);
        assert_eq!(d.db_1, 2);
        assert_eq!(d.tx_end2xlna_enable, 10);
        assert_eq!(d.thr_62, 28);
        assert_eq!(d.tx_end2xpa_disable, 5);
        assert_eq!(d.tx_frm2xpa_enable, 6);
        assert_eq!(d.pga_desired_size, 0xD6);
        assert_eq!(d.noise_floor_thr, -54);
        assert_eq!(d.cck_ofdm_power_delta, 20);
        assert_eq!(d.margin_tx_rx, 0x12);
        assert_eq!(d.i_cal, 0x1A);
        assert_eq!(d.q_cal, 0x0D);
        assert_eq!(d.cck_ofdm_gain_delta, -5);
    }

    #[test]
    fn rf2413_ob_db_patch_preserves_defaults_and_applies_bitswap() {
        // Default rfb_2413[23]=0x00280060, [24]=0x005000C3 encode OB=3, DB=3.
        let (d23, d24) = patch_rf2413_ob_db(0x0028_0060, 0x0050_00C3, 3, 3);
        assert_eq!(d23, 0x0028_0060);
        assert_eq!(d24, 0x0050_00C3);

        // OB=1 (bitswap3=4=0b100 -> bit0=0 in [23]:7, bits1..2=0b10 in [24]:0..1),
        // DB=2 (bitswap3=2=0b010 -> [23]:4..6 = 0x20).
        let (p23, p24) = patch_rf2413_ob_db(0x0028_0060, 0x0050_00C3, 1, 2);
        assert_eq!(p23, 0x0028_0020);
        assert_eq!(p24, 0x0050_00C2);
    }

    #[test]
    fn ofdm_timings_match_ath5k_formula_for_2462mhz() {
        // 2462 MHz (ch 11): coef_scaled = 1_677_721_600 / 2462 = 681_446 -> ilog2=19
        // coef_exp = 19, ds_coef_exp = 3, ds_coef_man = (681_446 + 16) >> 5 = 21_295 (0x532F).
        assert_eq!(compute_ofdm_timings(2462), Some((21_295, 3)));
        assert_eq!(compute_ofdm_timings(2412), Some((21_737, 3)));
        assert_eq!(compute_ofdm_timings(0), None);
    }

    #[test]
    fn rx_eth_classifier_filters_plaintext_echo_and_eapol() {
        let sta_mac = [0x00, 0x13, 0x46, 0xFE, 0x32, 0x0C];
        let peer_mac = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        let bcast = [0xFF; 6];

        // 1. Unicast ARP on WPA2 session: Protected=1 -> Deliver, Protected=0 -> Drop.
        let mut arp_ucast = [0u8; 42];
        arp_ucast[0..6].copy_from_slice(&sta_mac);
        arp_ucast[6..12].copy_from_slice(&peer_mac);
        arp_ucast[12..14].copy_from_slice(&[0x08, 0x06]);
        assert_eq!(
            classify_rx_eth_frame(&arp_ucast, &sta_mac, true, true),
            RxEthAction::Deliver
        );
        assert_eq!(
            classify_rx_eth_frame(&arp_ucast, &sta_mac, true, false),
            RxEthAction::Drop
        );
        // Same plaintext ARP on an open network -> Deliver.
        assert_eq!(
            classify_rx_eth_frame(&arp_ucast, &sta_mac, false, false),
            RxEthAction::Deliver
        );

        // 2. AP-reflected broadcast echo from our own STA MAC -> Drop.
        let mut self_bcast = [0u8; 42];
        self_bcast[0..6].copy_from_slice(&bcast);
        self_bcast[6..12].copy_from_slice(&sta_mac);
        self_bcast[12..14].copy_from_slice(&[0x08, 0x06]);
        assert_eq!(
            classify_rx_eth_frame(&self_bcast, &sta_mac, true, true),
            RxEthAction::Drop
        );

        // 3. Broadcast ARP from another LAN host -> Deliver.
        let mut peer_bcast = [0u8; 42];
        peer_bcast[0..6].copy_from_slice(&bcast);
        peer_bcast[6..12].copy_from_slice(&peer_mac);
        peer_bcast[12..14].copy_from_slice(&[0x08, 0x06]);
        assert_eq!(
            classify_rx_eth_frame(&peer_bcast, &sta_mac, true, true),
            RxEthAction::Deliver
        );

        // 4. Unicast EAPOL to our STA -> Eapol; broadcast EAPOL -> Drop.
        let mut eapol_ucast = [0u8; 20];
        eapol_ucast[0..6].copy_from_slice(&sta_mac);
        eapol_ucast[6..12].copy_from_slice(&peer_mac);
        eapol_ucast[12..14].copy_from_slice(&[0x88, 0x8E]);
        assert_eq!(
            classify_rx_eth_frame(&eapol_ucast, &sta_mac, true, true),
            RxEthAction::Eapol
        );
        eapol_ucast[0..6].copy_from_slice(&bcast);
        assert_eq!(
            classify_rx_eth_frame(&eapol_ucast, &sta_mac, true, true),
            RxEthAction::Drop
        );
    }

    #[test]
    fn encap_and_protect_eth_encrypts_group_m2_and_advances_pn_seq() {
        let bssid = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        let sta_mac = [0x00, 0x13, 0x46, 0xFE, 0x32, 0x0C];
        let tk = [0x5Au8; 16];
        let mut eth = [0u8; 14 + 99];
        eth[0..6].copy_from_slice(&bssid);
        eth[6..12].copy_from_slice(&sta_mac);
        eth[12..14].copy_from_slice(&[0x88, 0x8E]);
        eth[14] = 0x01;
        eth[15] = 0x03;

        let mut dot11 = [0u8; 256];
        let mut tx_seq: u16 = 10;
        let mut tx_pn: u64 = 7;

        let enc_len = encap_and_protect_eth(
            &mut dot11,
            &eth,
            &bssid,
            &sta_mac,
            true,
            &tk,
            &mut tx_seq,
            &mut tx_pn,
        )
        .unwrap();
        assert_eq!(tx_seq, 11);
        assert_eq!(tx_pn, 8);
        assert_eq!(enc_len, 24 + 8 + 8 + 99 + 8);
        assert_ne!(dot11[1] & 0x40, 0); // Protected bit set
        assert_eq!(dot11[24], 7); // PN0 == 7

        // Decrypt with the same PTK TK and verify the EAPOL payload survives intact.
        let dec_len = wifi_core::aes::ccmp_decrypt_frame(&tk, &mut dot11, enc_len).unwrap();
        assert_eq!(dec_len, 24 + 8 + 8 + 99);
        let (et, payload) = wifi_core::frame::snap_decap(&dot11[32..dec_len]).unwrap();
        assert_eq!(et, 0x888E);
        assert_eq!(payload, &eth[14..]);

        // Unprotected mode leaves Protected=0 and does not advance tx_pn.
        let open_len = encap_and_protect_eth(
            &mut dot11,
            &eth,
            &bssid,
            &sta_mac,
            false,
            &tk,
            &mut tx_seq,
            &mut tx_pn,
        )
        .unwrap();
        assert_eq!(tx_seq, 12);
        assert_eq!(tx_pn, 8);
        assert_eq!(open_len, 24 + 8 + 99);
        assert_eq!(dot11[1] & 0x40, 0);
    }

    #[test]
    fn tx_desc_36m_mrr_ladder_encodes_four_stages() {
        let ctl = build_5212_tx_ctl(1500, ATH5K_RATE_CODE_36M, None, false);
        assert_eq!((ctl.ctl2 >> 16) & 0xF, 3);
        assert_eq!((ctl.ctl2 >> 20) & 0xF, 3);
        assert_eq!((ctl.ctl2 >> 24) & 0xF, 3);
        assert_eq!((ctl.ctl2 >> 28) & 0xF, 5);
        assert_eq!(ctl.ctl3 & 0x1F, ATH5K_RATE_CODE_36M as u32);
        assert_eq!((ctl.ctl3 >> 5) & 0x1F, ATH5K_RATE_CODE_18M as u32);
        assert_eq!((ctl.ctl3 >> 10) & 0x1F, ATH5K_RATE_CODE_6M as u32);
        assert_eq!((ctl.ctl3 >> 15) & 0x1F, ATH5K_RATE_CODE_1M as u32);
    }
}
