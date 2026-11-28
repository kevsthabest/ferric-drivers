//! Polled AR2413 management/data transport. Caller owns the DMA lease.
//! Never reuse a descriptor until QCU idle is observed.
use crate::{registers::RegisterIo,engine,scan::ScanIo,dma::{Region,TX_DESC_OFF,TX_BUF_OFF,WIFI_DMA_SIZE,RX_DESC_COUNT,RX_DESC_STRIDE,RX_BUF_BASE,RX_BUF_SIZE},build_5212_tx_ctl,decode_5212_rx_status,encode_ccmp_key_words,connection::{LinkIo,Error}};
const AR5K_STA_ID1: usize = 0x8004;
const AR5K_STA_ID1_AP: u32 = 0x0001_0000;
const AR5K_STA_ID1_ADHOC: u32 = 0x0002_0000;
const AR5K_STA_ID1_PWR_SV: u32 = 0x0004_0000;
const AR5K_STA_ID1_NO_KEYSRCH: u32 = 0x0008_0000;
const AR5K_STA_ID1_DEFAULT_ANTENNA: u32 = 0x0020_0000;
const AR5K_STA_ID1_DESC_ANTENNA: u32 = 0x0040_0000;
const AR5K_STA_ID1_RTS_DEF_ANTENNA: u32 = 0x0080_0000;
const AR5K_STA_ID1_SELFGEN_DEF_ANT: u32 = 0x0400_0000;
const AR5K_STA_ID1_KEYSRCH_MODE: u32 = 0x1000_0000;
const AR5K_STA_ID1_BASE_RATE_11B: u32 = 0x0200_0000;
const AR5K_STA_ID1_ACKCTS_6MB: u32 = 0x0100_0000;
const AR5K_PHY_FAST_ANT_DIV: usize = 0xA208;
const AR5K_PHY_FAST_ANT_DIV_EN: u32 = 0x0000_2000;
const AR5K_PHY_AGCCTL: usize = 0x9860;
const AR5K_PHY_AGCCTL_OFDM_DIV_DIS: u32 = 0x0000_0008;
const AR5K_DEFAULT_ANTENNA: usize = 0x8058;
const AR5K_STA_ID0: usize = 0x8000;
const AR5K_BSSMSKL: usize = 0x80E0;
const AR5K_BSSMSKU: usize = 0x80E4;
const AR5K_BSS_ID0: usize = 0x8008;
const AR5K_BSS_ID1: usize = 0x800C;
const AR5K_BSS_ID1_AID_S: u32 = 16;
const AR5K_DIAG_SW_5211: usize = 0x8048;
const AR5K_DIAG_SW_DIS_ENC: u32 = 0x0000_0008;
const AR5K_DIAG_SW_DIS_DEC: u32 = 0x0000_0010;
const AR5K_KEYTABLE_SIZE: usize = 128;
const AR5K_KEYTABLE_0: usize = 0x8800;
const AR5K_SREV: usize = 0x4020;
const AR5K_KEYTABLE_TYPE_CLR: u32 = 0x0000_0007;
const AR5K_TXCFG: usize = 0x0030;
const AR5K_TXCFG_SDMAMR: u32 = 0x0000_0007;
const AR5K_TXCFG_TXFULL: u32 = 0x0000_03F0;
const AR5K_TXCFG_SDMAMR_128B: u32 = 0x0000_0005;
const AR5K_TXCFG_TXFULL_64B: u32 = 0x0000_0010;
const AR5K_DCU_LCL_IFS_0: usize = 0x1040;
const AR5K_DCU_RETRY_LMT_0: usize = 0x1080;
const AR5K_QCU_MISC_0: usize = 0x09C0;
const AR5K_DCU_MISC_0: usize = 0x1100;
const AR5K_DCU_MISC_FRAG_WAIT: u32 = 0x0000_0100;
const AR5K_DCU_QCUMASK_0: usize = 0x1000;
const AR5K_QUEUE_TXDP_0: usize = 0x0800;
pub fn ar2413_set_sta_bssid(
    io: &mut impl RegisterIo,
    sta_mac: &[u8; 6],
    bssid: &[u8; 6],
    aid: u16,
) {
    let sta_lo = u32::from_le_bytes([sta_mac[0], sta_mac[1], sta_mac[2], sta_mac[3]]);
    let sta_hi = u16::from_le_bytes([sta_mac[4], sta_mac[5]]) as u32;
    let bss_lo = u32::from_le_bytes([bssid[0], bssid[1], bssid[2], bssid[3]]);
    let bss_hi = u16::from_le_bytes([bssid[4], bssid[5]]) as u32;

    {
        let mut pcu_reg = io.read(AR5K_STA_ID1) & 0xFFFF_0000;
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

        let fast_div = io.read(AR5K_PHY_FAST_ANT_DIV);
        io.write(AR5K_PHY_FAST_ANT_DIV, fast_div | AR5K_PHY_FAST_ANT_DIV_EN);
        let agc = io.read(AR5K_PHY_AGCCTL);
        io.write(AR5K_PHY_AGCCTL, agc & !AR5K_PHY_AGCCTL_OFDM_DIV_DIS);
        io.write(AR5K_DEFAULT_ANTENNA, 1);

        io.write(AR5K_STA_ID0, sta_lo);
        io.write(AR5K_STA_ID1, pcu_reg | sta_hi);

        // Match all 48 bits of our STA MAC for hardware ACK generation.
        io.write(AR5K_BSSMSKL, 0xFFFF_FFFF);
        io.write(AR5K_BSSMSKU, 0x0000_FFFF);

        io.write(AR5K_BSS_ID0, bss_lo);
        io.write(AR5K_BSS_ID1,
            bss_hi | (((aid as u32) & 0x3FFF) << AR5K_BSS_ID1_AID_S),
        );

        // Ensure hardware encryption/decryption engines are enabled.
        let diag = io.read(AR5K_DIAG_SW_5211);
        io.write(AR5K_DIAG_SW_5211,
            diag & !(AR5K_DIAG_SW_DIS_ENC | AR5K_DIAG_SW_DIS_DEC),
        );
    }
}


pub fn ar2413_install_ccmp_key(
    io: &mut impl RegisterIo,
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
        { io.write(off + i * 4, words[i]) };
        i += 1;
    }
    let _ = { io.read(AR5K_SREV) };
}


pub fn ar2413_clear_keycache(io: &mut impl RegisterIo) {
    for slot in 0..AR5K_KEYTABLE_SIZE {
        let off = AR5K_KEYTABLE_0 + slot * 32;
        {
            io.write(off + 0, 0);
            io.write(off + 4, 0);
            io.write(off + 8, 0);
            io.write(off + 12, 0);
            io.write(off + 16, 0);
            io.write(off + 20, AR5K_KEYTABLE_TYPE_CLR);
            io.write(off + 24, 0);
            io.write(off + 28, 0);
        }
    }
}


pub fn ar2413_init_tx_q0(io: &mut impl RegisterIo) -> bool {
    {
        if engine::stop_tx(io).is_err(){return false;}
        // Configure TX DMA burst = 128B and FIFO trigger level = 64B (dma.c:857-858).
        let txcfg = io.read(AR5K_TXCFG);
        io.write(AR5K_TXCFG,
            (txcfg & !(AR5K_TXCFG_SDMAMR | AR5K_TXCFG_TXFULL))
                | AR5K_TXCFG_SDMAMR_128B
                | AR5K_TXCFG_TXFULL_64B,
        );

        // DCU 0 local IFS: cw_min=15, cw_max=1023, aifs=2 (qcu.c:340-344).
        let lcl_ifs = 15u32 | (1023u32 << 10) | (2u32 << 20);
        io.write(AR5K_DCU_LCL_IFS_0, lcl_ifs);

        // DCU 0 retry limits: rts=4, sta_rts=4, sta_data=14 (covers 4-stage MRR ladder).
        let retry_lmt = 4u32 | (4u32 << 8) | (14u32 << 14);
        io.write(AR5K_DCU_RETRY_LMT_0, retry_lmt);

        // QCU 0 misc: ASAP scheduling; DCU 0 misc: wait for next fragment (qcu.c:357-358).
        io.write(AR5K_QCU_MISC_0, 0);
        io.write(AR5K_DCU_MISC_0, AR5K_DCU_MISC_FRAG_WAIT);

        // Map QCU 0 -> DCU 0 (qcu.c:539) and clear TXDP(0).
        io.write(AR5K_DCU_QCUMASK_0, 1);
        io.write(AR5K_QUEUE_TXDP_0, 0);
    }
    true
}

pub trait TransportIo:ScanIo{
    fn copy_tx(&mut self,offset:usize,src:&[u8])->bool;
}
pub struct Transport<'a,T:TransportIo>{
    pub io:&'a mut T,pub region:Region,pub sta:[u8;6],pub ap:[u8;6],pub rx_index:usize,
}
impl<T:TransportIo> Transport<'_,T>{
    pub fn transmit(&mut self,frame:&[u8],rate:u8)->bool{
        if frame.is_empty()||frame.len()>WIFI_DMA_SIZE-TX_BUF_OFF||rate&31==0{return false;}
        if engine::wait_tx_idle(self.io).is_err(){return false;}
        if !self.io.copy_tx(TX_BUF_OFF,frame){return false;}
        let ctl=build_5212_tx_ctl(frame.len(),rate,None,false);
        let words=[0,self.region.physical+TX_BUF_OFF as u32,ctl.ctl0,ctl.ctl1,ctl.ctl2,ctl.ctl3,0,0];
        for (i,v) in words.into_iter().enumerate(){self.io.dma_write(TX_DESC_OFF+i*4,v);}
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        self.io.write(AR5K_QUEUE_TXDP_0,self.region.physical+TX_DESC_OFF as u32);
        self.io.write(0x0840,1);
        for _ in 0..100{
            let s=self.io.dma_read(TX_DESC_OFF+28);
            if s&1!=0{return self.io.dma_read(TX_DESC_OFF+24)&1!=0;}
            if !self.io.delay_ms(1){break;}
        }
        let _=engine::stop_tx(self.io);
        false
    }
    pub fn receive_raw(&mut self,out:&mut [u8],ms:u32)->Result<Option<usize>,Error>{
        for _ in 0..ms.max(1).min(100){
            let start=self.rx_index;
            for step in 0..RX_DESC_COUNT{
                let idx=(start+step)%RX_DESC_COUNT;let o=idx*RX_DESC_STRIDE;
                let s=self.io.dma_read(o+20);if s&1==0{continue;}
                let d=decode_5212_rx_status(self.io.dma_read(o+16),s);
                let len=usize::from(d.len);
                let valid=d.ok&&len>=4&&len<=RX_BUF_SIZE&&len<=out.len();
                let copied=valid&&self.io.copy_frame(RX_BUF_BASE+idx*RX_BUF_SIZE,&mut out[..len]);
                self.io.dma_write(o+12,RX_BUF_SIZE as u32);self.io.dma_write(o+16,0);self.io.dma_write(o+20,0);
                self.rx_index=(idx+1)%RX_DESC_COUNT;
                if valid&&!copied{return Err(Error::Transport);}
                if copied{return Ok(Some(len));}
            }
            if !self.io.delay_ms(1){return Err(Error::Transport);}
        }
        Ok(None)
    }
}
impl<T:TransportIo> LinkIo for Transport<'_,T>{
    fn send(&mut self,f:&[u8])->bool{self.transmit(f,0x1b)}
    fn receive(&mut self,dst:&mut [u8],ms:u32)->Result<Option<usize>,Error>{Ok(self.receive_raw(dst,ms)?.map(|n|n-4))}
    fn set_aid(&mut self,aid:u16)->bool{
        ar2413_set_sta_bssid(self.io,&self.sta,&self.ap,aid);
        self.io.read(AR5K_BSS_ID1)>>AR5K_BSS_ID1_AID_S==u32::from(aid)
    }
    fn install_keys(&mut self,tk:&[u8;16],gtk:&[u8;16],idx:u8)->bool{
        if !(1..=3).contains(&idx){return false;}
        let mut group=self.sta;group[0]|=1;
        ar2413_install_ccmp_key(self.io,4,tk,Some(&self.ap));
        ar2413_install_ccmp_key(self.io,idx,gtk,Some(&group));
        // Verify readable type/MAC fields without assuming key RAM is readable.
        for (slot,key,mac) in [(4,tk,self.ap),(idx,gtk,group)]{
            let words=encode_ccmp_key_words(key,Some(&mac));let o=AR5K_KEYTABLE_0+usize::from(slot)*32;
            if self.io.read(o+20)!=words[5]||self.io.read(o+24)!=words[6]||self.io.read(o+28)!=words[7]{return false;}
        }
        true
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    struct Mock{dma:[u32;4096],busy:bool,tx_copy:u32}
    impl RegisterIo for Mock{
        fn read(&mut self,o:usize)->u32{if o==0x840&&self.busy{1}else{0}}
        fn write(&mut self,o:usize,v:u32){if o==0x840&&v==1{self.dma[(TX_DESC_OFF+24)/4]=1;self.dma[(TX_DESC_OFF+28)/4]=1;}}
        fn delay_ms(&mut self,_:u32)->bool{true}
    }
    impl ScanIo for Mock{
        fn dma_read(&mut self,o:usize)->u32{self.dma[o/4]}
        fn dma_write(&mut self,o:usize,v:u32){self.dma[o/4]=v;}
        fn copy_frame(&mut self,_:usize,dst:&mut [u8])->bool{dst.fill(0x42);true}
    }
    impl TransportIo for Mock{fn copy_tx(&mut self,_:usize,_:&[u8])->bool{self.tx_copy+=1;true}}
    #[test]fn busy_queue_never_touches_descriptor(){
        let mut m=Mock{dma:[0;4096],busy:true,tx_copy:0};
        let mut t=Transport{io:&mut m,region:Region::new(0x20000,0x40000).unwrap(),sta:[2;6],ap:[4;6],rx_index:0};
        assert!(!t.transmit(&[1;60],0x1b));assert_eq!(m.tx_copy,0);
    }
    #[test]fn transmit_completion(){
        let mut m=Mock{dma:[0;4096],busy:false,tx_copy:0};
        let mut t=Transport{io:&mut m,region:Region::new(0x20000,0x40000).unwrap(),sta:[2;6],ap:[4;6],rx_index:0};
        assert!(t.transmit(&[1;60],0x1b));assert_eq!(m.tx_copy,1);assert_eq!(m.dma[(TX_DESC_OFF+4)/4],0x43800);
    }
}

pub fn install_group_checked(io:&mut impl RegisterIo,slot:u8,key:&[u8;16],mac:&[u8;6])->bool{
    if !(1..=3).contains(&slot)||mac[0]&1==0||io.read(AR5K_SREV)==u32::MAX{return false;}
    ar2413_install_ccmp_key(io,slot,key,Some(mac));
    let w=encode_ccmp_key_words(key,Some(mac));let o=AR5K_KEYTABLE_0+usize::from(slot)*32;
    io.read(o+20)==w[5]&&io.read(o+24)==w[6]&&io.read(o+28)==w[7]
}
