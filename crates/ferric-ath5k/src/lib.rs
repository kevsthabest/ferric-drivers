//! Shared AR2413 EEPROM, RF and descriptor encoding. Extracted from the existing Ferric backend.
#![no_std]
#![forbid(unsafe_code)]
#![deny(warnings)]

pub mod registers;
pub mod initvals;
pub mod radio;
pub mod dma;
pub mod engine;

const AR5K_SREV_AR5212_V4: u32 = 0x54;
const AR5K_EEPROM_VERSION_5_0: u16 = 0x5000;
const AR5K_EEPROM_AES_DIS_BIT: u16 = 0x0002;
const AR5K_RX_STAT1_DONE: u32 = 0x0000_0001;
pub const AR5K_RX_STAT1_OK: u32 = 0x0000_0002;
const AR5K_RX_STAT0_DATA_LEN: u32 = 0x0000_0FFF;
const AR5K_RX_STAT0_RSSI_SHIFT: u32 = 20;
const ATH5K_RATE_CODE_1M: u8 = 0x1B;
pub const AR5K_4W_TX_DESC_CTL0_FRAME_LEN: u32 = 0x0000_0FFF;
pub const AR5K_4W_TX_DESC_CTL0_XMIT_POWER_S: u32 = 16;
pub const AR5K_4W_TX_DESC_CTL0_XMIT_POWER: u32 = 0x003F_0000;
pub const AR5K_4W_TX_DESC_CTL0_CLRDMASK: u32 = 0x0100_0000;
pub const AR5K_4W_TX_DESC_CTL1_BUF_LEN: u32 = 0x0000_0FFF;
pub const AR5K_4W_TX_DESC_CTL0_ENCRYPT_KEY_VALID: u32 = 0x4000_0000;
pub const AR5K_4W_TX_DESC_CTL1_ENCRYPT_KEY_IDX_S: u32 = 13;
pub const AR5K_4W_TX_DESC_CTL1_ENCRYPT_KEY_IDX: u32 = 0x000F_E000;
pub const AR5K_4W_TX_DESC_CTL1_NOACK: u32 = 0x0100_0000;
const ATH5K_RATE_CODE_36M: u8 = 0x0D;
pub const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0_S: u32 = 16;
pub const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES0: u32 = 0x000F_0000;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES1_S: u32 = 20;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES1: u32 = 0x00F0_0000;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES2_S: u32 = 24;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES2: u32 = 0x0F00_0000;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES3_S: u32 = 28;
const AR5K_4W_TX_DESC_CTL2_XMIT_TRIES3: u32 = 0xF000_0000;
pub const AR5K_4W_TX_DESC_CTL3_XMIT_RATE0: u32 = 0x0000_001F;
const ATH5K_RATE_CODE_18M: u8 = 0x0E;
const AR5K_4W_TX_DESC_CTL3_XMIT_RATE1_S: u32 = 5;
const ATH5K_RATE_CODE_6M: u8 = 0x0B;
const AR5K_4W_TX_DESC_CTL3_XMIT_RATE2_S: u32 = 10;
const AR5K_4W_TX_DESC_CTL3_XMIT_RATE3_S: u32 = 15;
const AR5K_KEYTABLE_TYPE_CCM: u32 = 0x0000_0006;
const AR5K_KEYTABLE_VALID: u32 = 0x0000_8000;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TxDesc4W {
    pub ctl0: u32,
    pub ctl1: u32,
    pub ctl2: u32,
    pub ctl3: u32,
}


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


#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct EepromAnts {
    pub switch_settling: u32,
    pub atn_tx_rx: u32,
    pub ant_idle: u32,
    pub swtable_a: u32,
    pub swtable_b: u32,
}


#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct RxDescDecode {
    pub done: bool,
    pub ok: bool,
    pub len: u16,
    pub rssi: u8,
}


pub const fn decode_aes_support(srev: u32, ee_version: u16, ee_misc5: u16) -> bool {
    let mac_srev = srev & 0xFF;
    mac_srev >= AR5K_SREV_AR5212_V4
        && (ee_version < AR5K_EEPROM_VERSION_5_0 || (ee_misc5 & AR5K_EEPROM_AES_DIS_BIT) == 0)
}


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


pub const fn decode_5212_rx_status(stat0: u32, stat1: u32) -> RxDescDecode {
    RxDescDecode {
        done: (stat1 & AR5K_RX_STAT1_DONE) != 0,
        ok: (stat1 & AR5K_RX_STAT1_OK) != 0,
        len: (stat0 & AR5K_RX_STAT0_DATA_LEN) as u16,
        rssi: ((stat0 >> AR5K_RX_STAT0_RSSI_SHIFT) & 0xFF) as u8,
    }
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


pub const fn patch_rf2413_ob_db(val23: u32, val24: u32, ob: u8, db: u8) -> (u32, u32) {
    let ob_swap = (((ob & 1) << 2) | (ob & 2) | ((ob >> 2) & 1)) as u32;
    let db_swap = (((db & 1) << 2) | (db & 2) | ((db >> 2) & 1)) as u32;
    // DB_2GHZ: len=3, pos=165 (entry 23, bits 4..=6).
    // OB_2GHZ: len=3, pos=168 (entry 23 bit 7 + entry 24 bits 0..=1).
    let out23 = (val23 & !0x0000_00F0) | ((db_swap << 4) & 0x0000_0070) | ((ob_swap & 1) << 7);
    let out24 = (val24 & !0x0000_0003) | ((ob_swap >> 1) & 0x0000_0003);
    (out23, out24)
}


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

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn captured_card_identity() {
        assert!(decode_aes_support(0x78,0x5001,5));
        assert_eq!(decode_eeprom_mac(0x0013,0x46d7,0x848a),Some([0,0x13,0x46,0xd7,0x84,0x8a]));
        assert_eq!(decode_eeprom_mac(0,0,0),None);
    }
    #[test] fn channel_bounds() {
        for ch in 0..13 { assert!(rf5112_channel_regs(2412+5*ch).is_some()); }
        assert!(rf5112_channel_regs(2484).is_none());
        assert!(compute_ofdm_timings(0).is_none());
    }
}
