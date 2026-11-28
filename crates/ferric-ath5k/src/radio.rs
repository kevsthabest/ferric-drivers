//! Shared AR2413 RF initialization and channel tuning, preserving legacy ordering.
use crate::{decode_eeprom_ants,decode_eeprom_modes_11g,rf5112_channel_regs,patch_rf2413_ob_db,compute_ofdm_timings};
use crate::registers::{RegisterIo,wake_chip,nic_reset,eeprom_read};
use crate::initvals::{AR2413_11G_INITVALS,RF2413_11G_RFBANKS,RF2413_2GHZ_RFGAIN};
const AR5K_SREV: usize = 0x4020;
const AR5K_PHY_ADC_CTL: usize = 0x982C;
const AR5K_PHY_DAG_CCK_CTL: usize = 0xA228;
const AR5K_PHY_DAG_CCK_CTL_EN_RSSI_THR: u32 = 0x0000_0200;
const AR5K_PHY_DAG_CCK_CTL_RSSI_THR: u32 = 0x0001_FC00;
const AR5K_PHY_DAG_CCK_CTL_RSSI_THR_S: u32 = 10;
const AR5K_SEQ_MASK: usize = 0x8060;
const AR5K_PHY_BLUETOOTH: usize = 0xA254;
const AR5K_TXCFG: usize = 0x0030;
const AR5K_TXCFG_DCU_DBL_BUF_DIS: u32 = 0x0000_8000;
const AR5K_PHY_TX_PWR_ADJ: usize = 0x994C;
const AR5K_PHY_TX_PWR_ADJ_CCK_GAIN_DELTA_S: u32 = 6;
const AR5K_PHY_TX_PWR_ADJ_CCK_GAIN_DELTA: u32 = 0x0000_0FC0;
const AR5K_PHY_TX_PWR_ADJ_CCK_PCDAC_INDEX_S: u32 = 18;
const AR5K_PHY_TX_PWR_ADJ_CCK_PCDAC_INDEX: u32 = 0x00FC_0000;
const AR5K_PHY_ANT_CTL: usize = 0x9910;
const AR5K_PHY_ANT_CTL_SWTABLE_IDLE: u32 = 0x0000_03F0;
const AR5K_PHY_ANT_CTL_SWTABLE_IDLE_S: u32 = 4;
const AR5K_PHY_ANT_CTL_TXRX_EN: u32 = 0x0000_0001;
const AR5K_PHY_ANT_SWITCH_TABLE_0: usize = 0x9960;
const AR5K_PHY_ANT_SWITCH_TABLE_1: usize = 0x9964;
const AR5K_PHY_NFTHRES: usize = 0x9968;
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
const AR5K_PHY_GAIN_2GHZ: usize = 0xA20C;
const AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX: u32 = 0x00FC_0000;
const AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX_S: u32 = 18;
const AR5K_PHY_RF_CTL4: usize = 0x9834;
const AR5K_PHY_RF_CTL3: usize = 0x9828;
const AR5K_PHY_RF_CTL3_TXE2XLNA_ON: u32 = 0x0000_FF00;
const AR5K_PHY_RF_CTL3_TXE2XLNA_ON_S: u32 = 8;
const AR5K_PHY_NF: usize = 0x9864;
const AR5K_PHY_NF_THRESH62: u32 = 0x0007_F000;
const AR5K_PHY_NF_THRESH62_S: u32 = 12;
const AR5K_PHY_IQ: usize = 0x9920;
const AR5K_PHY_IQ_CORR_Q_I_COFF: u32 = 0x0000_07E0;
const AR5K_PHY_IQ_CORR_Q_Q_COFF: u32 = 0x0000_001F;
const AR5K_PHY_IQ_CORR_Q_I_COFF_S: u32 = 5;
const AR5K_PHY_IQ_CORR_ENABLE: u32 = 0x0000_0800;
const AR5K_PHY_HEAVY_CLIP_ENABLE: usize = 0x99E0;
const AR5K_RESET_CTL_PCU: u32 = 0x0000_0001;
const AR5K_RESET_CTL_BASEBAND: u32 = 0x0000_0002;
const AR5K_RESET_CTL_PCI: u32 = 0x0000_0010;
const AR5K_PCICFG: usize = 0x4010;
const AR5K_PCICFG_RETRY_FIX: u32 = 0x0000_1000;
const AR5K_PHY_PLL: usize = 0x987C;
const AR5K_PHY_PLL_RF5112_44MHZ: u32 = 0x0000_0040 | 0x0000_00AB;
const AR5K_PHY_MODE: usize = 0xA200;
const AR5K_PHY_MODE_11G_2GHZ: u32 = 0x0000_0008 | 0x0000_0002 | 0x0000_0004;
const AR5K_PHY_TURBO: usize = 0x9804;
const AR5K_PHY_BASE: usize = 0x9800;
const AR5K_PHY_SHIFT_5GHZ: u32 = 0x0000_0007;
const AR5K_PHY_TIMING_3: usize = 0x9814;
const AR5K_PHY_TIMING_3_DSC_MAN: u32 = 0xFFFE_0000;
const AR5K_PHY_TIMING_3_DSC_EXP: u32 = 0x0001_E000;
const AR5K_PHY_TIMING_3_DSC_MAN_S: u32 = 17;
const AR5K_PHY_TIMING_3_DSC_EXP_S: u32 = 13;
const AR5K_RF_BUFFER: usize = 0x989C;
const AR5K_RF_BUFFER_CONTROL_5: usize = 0x98D8;
const AR5K_PHY_ACT: usize = 0x981C;
const AR5K_PHY_ACT_ENABLE: u32 = 0x0000_0001;
const AR5K_PHY_TST1: usize = 0x9808;
const AR5K_PHY_TST1_TXHOLD: u32 = 0x0000_3800;
const AR5K_PHY_ADC_TEST: usize = 0x9C24;
const AR5K_PHY_AGCCTL: usize = 0x9860;
const AR5K_PHY_AGCCTL_CAL: u32 = 0x0000_0001;
const AR5K_PHY_AGCCTL_NF: u32 = 0x0000_0002;
const AR5K_MCAST_FILTER0_5211: usize = 0x8040;
const AR5K_MCAST_FILTER1_5211: usize = 0x8044;
const AR5K_RX_FILTER_5211: usize = 0x803C;
const AR5K_RX_FILTER_BEACON: u32 = 0x0000_0010;
const AR5K_RX_FILTER_BCAST: u32 = 0x0000_0004;
const AR5K_RX_FILTER_MCAST: u32 = 0x0000_0002;
const AR5K_RX_FILTER_UCAST: u32 = 0x0000_0001;
const AR5K_DIAG_SW_5211: usize = 0x8048;
const AR5K_DIAG_SW_DIS_RX_5211: u32 = 0x0000_0020;
const AR5K_RXCFG: usize = 0x0034;

pub fn write_reg_table(io: &mut impl RegisterIo, table: &[(u16, u32)]) {
    for (i, &(reg, val)) in table.iter().enumerate() {
        if (i & 7) == 7 {
            let _ = { io.read(AR5K_SREV) };
        }
        { io.write(reg as usize, val) };
    }
}


pub fn ar2413_verify_eeprom_checksum(io: &mut impl RegisterIo, ee_ver: u16) -> bool {
    if ee_ver < 0x3000 {
        return true;
    }
    const INFO_BASE: u32 = 0x00C0;
    const INFO_MAX: u32 = 0x0400 - INFO_BASE;
    let mut eep_max = INFO_MAX;
    let Some(upper) = ({ eeprom_read(io, 0x001C) }) else {
        return false;
    };
    if upper != 0 {
        let Some(lower) = ({ eeprom_read(io, 0x001B) }) else {
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
        let Some(val) = ({ eeprom_read(io, INFO_BASE + off) }) else {
            return false;
        };
        cksum ^= val;
    }
    cksum == 0xFFFF
}


pub fn ar2413_commit_eeprom_11g(io: &mut impl RegisterIo) -> Option<(u8, u8)> {
    {
        // AR5212A+ PHY/MAC tweaks (reset.c:851-885).
        io.write(AR5K_PHY_ADC_CTL, 0x0002_A002);
        let dag = io.read(AR5K_PHY_DAG_CCK_CTL);
        io.write(AR5K_PHY_DAG_CCK_CTL,
            (dag & !(AR5K_PHY_DAG_CCK_CTL_EN_RSSI_THR | AR5K_PHY_DAG_CCK_CTL_RSSI_THR))
                | ((2u32 << AR5K_PHY_DAG_CCK_CTL_RSSI_THR_S) & AR5K_PHY_DAG_CCK_CTL_RSSI_THR),
        );
        io.write(AR5K_SEQ_MASK, 0x0000_000F);
        io.write(AR5K_PHY_BLUETOOTH, 0);
        let txcfg = io.read(AR5K_TXCFG);
        io.write(AR5K_TXCFG, txcfg & !AR5K_TXCFG_DCU_DBL_BUF_DIS);

        let ee_ver = eeprom_read(io, 0x00C1)?;
        if !ar2413_verify_eeprom_checksum(io, ee_ver) {
            return None;
        }
        let off = if ee_ver >= 0x3003 { 0x010Du32 } else { 0x00DAu32 };
        let mut w = [0u16; 5];
        for i in 0..5 {
            w[i] = eeprom_read(io, off + i as u32)?;
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
            m[i] = eeprom_read(io, off + 5 + i as u32)?;
        }
        let modes = decode_eeprom_modes_11g(ee_ver, &m);

        // CCK-to-OFDM power/gain delta (reset.c:995-1007).
        let cck_pwr_delta = ((modes.cck_ofdm_power_delta as i32) * 2) / 10;
        let gain_neg = (-(modes.cck_ofdm_gain_delta as i32)) as u32;
        let pcdac_neg = (-cck_pwr_delta) as u32;
        io.write(AR5K_PHY_TX_PWR_ADJ,
            ((gain_neg << AR5K_PHY_TX_PWR_ADJ_CCK_GAIN_DELTA_S)
                & AR5K_PHY_TX_PWR_ADJ_CCK_GAIN_DELTA)
                | ((pcdac_neg << AR5K_PHY_TX_PWR_ADJ_CCK_PCDAC_INDEX_S)
                    & AR5K_PHY_TX_PWR_ADJ_CCK_PCDAC_INDEX),
        );

        // Program PCB RF antenna switch tables & idle control (phy.c:2270-2280).
        let ant_ctl = io.read(AR5K_PHY_ANT_CTL);
        io.write(AR5K_PHY_ANT_CTL,
            (ant_ctl & !AR5K_PHY_ANT_CTL_SWTABLE_IDLE)
                | ((ants.ant_idle << AR5K_PHY_ANT_CTL_SWTABLE_IDLE_S)
                    & AR5K_PHY_ANT_CTL_SWTABLE_IDLE)
                | AR5K_PHY_ANT_CTL_TXRX_EN,
        );
        io.write(AR5K_PHY_ANT_SWITCH_TABLE_0, ants.swtable_a);
        io.write(AR5K_PHY_ANT_SWITCH_TABLE_1, ants.swtable_b);

        // Noise floor threshold (reset.c:1022-1025).
        let nf_sval = (((modes.noise_floor_thr as i32) as u32) & 0x01FF) | (1 << 9);
        io.write(AR5K_PHY_NFTHRES, nf_sval);

        // Switch settling time & TX/RX attenuation (reset.c:1055-1062).
        let settling = io.read(AR5K_PHY_SETTLING);
        io.write(AR5K_PHY_SETTLING,
            (settling & !AR5K_PHY_SETTLING_SWITCH)
                | ((ants.switch_settling << AR5K_PHY_SETTLING_SWITCH_S) & AR5K_PHY_SETTLING_SWITCH),
        );
        let gain = io.read(AR5K_PHY_GAIN);
        io.write(AR5K_PHY_GAIN,
            (gain & !AR5K_PHY_GAIN_TXRX_ATTEN)
                | ((ants.atn_tx_rx << AR5K_PHY_GAIN_TXRX_ATTEN_S) & AR5K_PHY_GAIN_TXRX_ATTEN),
        );

        // ADC / PGA desired levels (reset.c:1064-1071).
        let des = io.read(AR5K_PHY_DESIRED_SIZE);
        io.write(AR5K_PHY_DESIRED_SIZE,
            (des & !(AR5K_PHY_DESIRED_SIZE_ADC | AR5K_PHY_DESIRED_SIZE_PGA))
                | ((modes.adc_desired_size as u32) & AR5K_PHY_DESIRED_SIZE_ADC)
                | (((modes.pga_desired_size as u32) << AR5K_PHY_DESIRED_SIZE_PGA_S)
                    & AR5K_PHY_DESIRED_SIZE_PGA),
        );

        // TX/RX margin (reset.c:1074-1077).
        if ee_ver >= 0x4001 {
            let g2 = io.read(AR5K_PHY_GAIN_2GHZ);
            io.write(AR5K_PHY_GAIN_2GHZ,
                (g2 & !AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX)
                    | (((modes.margin_tx_rx as u32) << AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX_S)
                        & AR5K_PHY_GAIN_2GHZ_MARGIN_TXRX),
            );
        }

        // External PA (XPA) enable/disable turnaround timing (reset.c:1080-1085).
        let xpa_dis = modes.tx_end2xpa_disable as u32;
        let xpa_en = modes.tx_frm2xpa_enable as u32;
        io.write(AR5K_PHY_RF_CTL4,
            (xpa_dis << 24) | (xpa_dis << 16) | (xpa_en << 8) | xpa_en,
        );

        // External LNA (XLNA) turnaround timing (reset.c:1087-1090).
        let ctl3 = io.read(AR5K_PHY_RF_CTL3);
        io.write(AR5K_PHY_RF_CTL3,
            (ctl3 & !AR5K_PHY_RF_CTL3_TXE2XLNA_ON)
                | (((modes.tx_end2xlna_enable as u32) << AR5K_PHY_RF_CTL3_TXE2XLNA_ON_S)
                    & AR5K_PHY_RF_CTL3_TXE2XLNA_ON),
        );

        // Thresh62 (reset.c:1092-1095).
        let nf = io.read(AR5K_PHY_NF);
        io.write(AR5K_PHY_NF,
            (nf & !AR5K_PHY_NF_THRESH62)
                | (((modes.thr_62 as u32) << AR5K_PHY_NF_THRESH62_S) & AR5K_PHY_NF_THRESH62),
        );

        // I/Q correction coefficients — enable bit set last (reset.c:1110-1118).
        if ee_ver >= 0x4000 {
            let iq = io.read(AR5K_PHY_IQ);
            let iq = (iq & !(AR5K_PHY_IQ_CORR_Q_I_COFF | AR5K_PHY_IQ_CORR_Q_Q_COFF))
                | (((modes.i_cal as u32) << AR5K_PHY_IQ_CORR_Q_I_COFF_S)
                    & AR5K_PHY_IQ_CORR_Q_I_COFF)
                | ((modes.q_cal as u32) & AR5K_PHY_IQ_CORR_Q_Q_COFF);
            io.write(AR5K_PHY_IQ, iq);
            io.write(AR5K_PHY_IQ, iq | AR5K_PHY_IQ_CORR_ENABLE);
        }

        // Disable heavy clipping on EEPROM >= 5.1 (reset.c:1120-1122).
        if ee_ver >= 0x5001 {
            io.write(AR5K_PHY_HEAVY_CLIP_ENABLE, 0);
        }
        Some((modes.ob_1, modes.db_1))
    }
}


pub fn ar2413_radio_init_11g(io: &mut impl RegisterIo, freq_mhz: u16, last_fail: &mut u32) -> bool {
    let Some((rf_lo, rf_hi)) = rf5112_channel_regs(freq_mhz) else {
        return false;
    };
    if !{ wake_chip(io) } {
        return false;
    }
    if !{
        nic_reset(io,
            AR5K_RESET_CTL_PCU | AR5K_RESET_CTL_BASEBAND | AR5K_RESET_CTL_PCI,
        )
    } {
        return false;
    }
    if !{ wake_chip(io) } {
        return false;
    }
    if !{ nic_reset(io, 0) } {
        return false;
    }
    {
        let pcfg = io.read(AR5K_PCICFG);
        io.write(AR5K_PCICFG, pcfg | AR5K_PCICFG_RETRY_FIX);

        // 2.4 GHz 802.11g PLL + PHY mode (ath5k/reset.c:747-826).
        io.write(AR5K_PHY_PLL, AR5K_PHY_PLL_RF5112_44MHZ);
        if !io.delay_ms(1){*last_fail=9;return false;}
        io.write(AR5K_PHY_MODE, AR5K_PHY_MODE_11G_2GHZ);
        io.write(AR5K_PHY_TURBO, 0);

        // PHY access + AR2413 802.11g initvals + EEPROM RF switch/PHY calibration + RF2413 gain + RF banks.
        io.write(AR5K_PHY_BASE, AR5K_PHY_SHIFT_5GHZ);
        write_reg_table(io, AR2413_11G_INITVALS);
        let Some((ob_1, db_1)) = ar2413_commit_eeprom_11g(io) else {
            *last_fail = 4;
            return false;
        };
        // Write channel-specific OFDM pilot-tracking delta slope (phy.c:286-344,3799).
        if let Some((dsc_man, dsc_exp)) = compute_ofdm_timings(freq_mhz) {
            let t3 = io.read(AR5K_PHY_TIMING_3);
            io.write(AR5K_PHY_TIMING_3,
                (t3 & !(AR5K_PHY_TIMING_3_DSC_MAN | AR5K_PHY_TIMING_3_DSC_EXP))
                    | ((dsc_man << AR5K_PHY_TIMING_3_DSC_MAN_S) & AR5K_PHY_TIMING_3_DSC_MAN)
                    | ((dsc_exp << AR5K_PHY_TIMING_3_DSC_EXP_S) & AR5K_PHY_TIMING_3_DSC_EXP),
            );
        }
        write_reg_table(io, RF2413_2GHZ_RFGAIN);
        if !io.delay_ms(1){*last_fail=9;return false;}
        // Patch EEPROM 2.4 GHz OB/DB bias into Bank 6 entries 23..=24 before upload (phy.c:935-940).
        let (p23, p24) = patch_rf2413_ob_db(
            RF2413_11G_RFBANKS[23].1,
            RF2413_11G_RFBANKS[24].1,
            ob_1,
            db_1,
        );
        for (i, &(reg, val)) in RF2413_11G_RFBANKS.iter().enumerate() {
            if (i & 7) == 7 {
                let _ = io.read(AR5K_SREV);
            }
            let out_val = if i == 23 {
                p23
            } else if i == 24 {
                p24
            } else {
                val
            };
            io.write(reg as usize, out_val);
        }

        // Program channel frequency and activate PHY.
        io.write(AR5K_RF_BUFFER, rf_lo);
        io.write(AR5K_RF_BUFFER_CONTROL_5, rf_hi);
        io.write(AR5K_PHY_ACT, AR5K_PHY_ACT_ENABLE);
        if !io.delay_ms(5){*last_fail=9;return false;}

        // ADC ready check (ath5k/phy.c:3896-3903).
        let tst1 = io.read(AR5K_PHY_TST1);
        io.write(AR5K_PHY_TST1, AR5K_PHY_TST1_TXHOLD);
        let mut adc_ok = false;
        for _ in 0..20 {
            if (io.read(AR5K_PHY_ADC_TEST) & 0x10) == 0 {
                adc_ok = true;
                break;
            }
            if !io.delay_ms(1){io.write(AR5K_PHY_TST1,tst1);*last_fail=9;return false;}
        }
        io.write(AR5K_PHY_TST1, tst1);
        if !adc_ok {
            *last_fail = 5;
            return false;
        }

        // AGC + noise-floor calibration (ath5k/phy.c:3924-3944).
        let agc = io.read(AR5K_PHY_AGCCTL);
        io.write(AR5K_PHY_AGCCTL,
            agc | AR5K_PHY_AGCCTL_CAL | AR5K_PHY_AGCCTL_NF,
        );
        let mut agc_ok = false;
        for _ in 0..50 {
            if (io.read(AR5K_PHY_AGCCTL) & AR5K_PHY_AGCCTL_CAL) == 0 {
                agc_ok = true;
                break;
            }
            if !io.delay_ms(1){*last_fail=9;return false;}
        }
        if !agc_ok {
            *last_fail = 6;
            return false;
        }

        // Configure RX filters (beacons + broadcast + multicast + unicast) and enable RX.
        io.write(AR5K_MCAST_FILTER0_5211, 0xFFFF_FFFF);
        io.write(AR5K_MCAST_FILTER1_5211, 0xFFFF_FFFF);
        io.write(AR5K_RX_FILTER_5211,
            AR5K_RX_FILTER_BEACON
                | AR5K_RX_FILTER_BCAST
                | AR5K_RX_FILTER_MCAST
                | AR5K_RX_FILTER_UCAST,
        );
        let diag = io.read(AR5K_DIAG_SW_5211);
        io.write(AR5K_DIAG_SW_5211, diag & !AR5K_DIAG_SW_DIS_RX_5211);
        io.write(AR5K_RXCFG, 0x0005); // 128-byte DMA burst
    }
    true
}


pub fn ar2413_set_channel_2g(io: &mut impl RegisterIo, freq_mhz: u16) -> bool {
    let Some((rf_lo, rf_hi)) = rf5112_channel_regs(freq_mhz) else {
        return false;
    };
    {
        io.write(AR5K_RF_BUFFER, rf_lo);
        io.write(AR5K_RF_BUFFER_CONTROL_5, rf_hi);
        if !io.delay_ms(3){return false;}
        let agc = io.read(AR5K_PHY_AGCCTL);
        io.write(AR5K_PHY_AGCCTL,
            agc | AR5K_PHY_AGCCTL_CAL | AR5K_PHY_AGCCTL_NF,
        );
        for _ in 0..15 {
            if (io.read(AR5K_PHY_AGCCTL) & AR5K_PHY_AGCCTL_CAL) == 0 {
                return true;
            }
            if !io.delay_ms(1){return false;}
        }
    }
    false
}
