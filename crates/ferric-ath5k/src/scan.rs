//! Passive 2.4 GHz scanner with injected register and DMA interfaces.
//! The caller must own a live DMA lease, and must shut it down on every
//! result, including failure. No packet transmission is performed.
use crate::{dma::{Region,initialize_rx,RX_DESC_COUNT,RX_DESC_STRIDE,RX_BUF_BASE,RX_BUF_SIZE},engine,radio,registers::RegisterIo,decode_5212_rx_status};
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

    pub fn record_beacon(&mut self, b: &wifi_core::frame::BeaconInfo<'_>, ch: u8, rssi: u8) {
        let count = (self.ap_count as usize).min(self.entries.len());
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
pub trait ScanIo:RegisterIo{
    fn dma_read(&mut self,offset:usize)->u32;
    fn dma_write(&mut self,offset:usize,value:u32);
    /// Copy a completed DMA frame to a caller-owned buffer before recycling.
    fn copy_frame(&mut self,offset:usize,dst:&mut [u8])->bool;
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum Error{Stop,Tune,Start,Timer,Buffer}
pub fn passive_scan(io:&mut impl ScanIo,r:Region,res:&mut WifiScanResult)->Result<(),Error>{
    *res=WifiScanResult::ZERO;res.present=1;res.stage=4;
    let mut frame=[0u8;RX_BUF_SIZE];
    let result=(||{
        for ch in 1u8..=11{
            engine::stop_rx(io).map_err(|_|Error::Stop)?;
            if ch>1 && !radio::ar2413_set_channel_2g(io,2407+5*u16::from(ch)){return Err(Error::Tune);}
            initialize_rx(r,|o,v|io.dma_write(o,v));
            if !engine::start_rx(io,r.physical){return Err(Error::Start);}
            let mut idx=0;
            for _ in 0..150{
                if !io.delay_ms(1){return Err(Error::Timer);}
                let start=idx;
                for step in 0..RX_DESC_COUNT{
                    let cur=(start+step)%RX_DESC_COUNT;let o=cur*RX_DESC_STRIDE;
                    let stat1=io.dma_read(o+20);
                    if stat1&1==0{continue;}
                    let dec=decode_5212_rx_status(io.dma_read(o+16),stat1);
                    res.rx_total=res.rx_total.wrapping_add(1);
                    if dec.ok&&dec.len>=36&&usize::from(dec.len)<=RX_BUF_SIZE{
                        res.rx_ok=res.rx_ok.wrapping_add(1);
                        let len=usize::from(dec.len);
                        if !io.copy_frame(RX_BUF_BASE+cur*RX_BUF_SIZE,&mut frame[..len]){return Err(Error::Buffer);}
                        if let Some(b)=wifi_core::frame::parse_beacon(&frame[..len]){
                            res.beacons=res.beacons.wrapping_add(1);res.record_beacon(&b,ch,dec.rssi);
                        }
                    }
                    io.dma_write(o+12,RX_BUF_SIZE as u32);io.dma_write(o+16,0);io.dma_write(o+20,0);
                    idx=(cur+1)%RX_DESC_COUNT;
                }
            }
        }
        Ok(())
    })();
    engine::stop_rx(io).map_err(|_|Error::Stop)?;
    result?;
    res.stage=5;Ok(())
}
#[cfg(test)]
mod tests{
    use super::*;
    struct Mock{regs:[u32;4],dma:[u32;4096],starts:u32,delay_ok:bool}
    impl RegisterIo for Mock{
        fn read(&mut self,o:usize)->u32{match o{8=>self.regs[0],12=>self.regs[1],_=>0}}
        fn write(&mut self,o:usize,v:u32){match o{8=>{self.regs[0]=if v==4{self.starts+=1;4}else{0};},12=>self.regs[1]=v,_=>{}}}
        fn delay_ms(&mut self,_:u32)->bool{self.delay_ok}
    }
    impl ScanIo for Mock{
        fn dma_read(&mut self,o:usize)->u32{self.dma[o/4]}
        fn dma_write(&mut self,o:usize,v:u32){self.dma[o/4]=v;}
        fn copy_frame(&mut self,_:usize,_:&mut [u8])->bool{false}
    }
    fn fixture()->Mock{Mock{regs:[0;4],dma:[0;4096],starts:0,delay_ok:true}}
    #[test]fn empty_scan_visits_eleven_channels_and_stops(){let mut m=fixture();let mut s=WifiScanResult::ZERO;assert_eq!(passive_scan(&mut m,Region::new(0x20000,0x30000).unwrap(),&mut s),Ok(()));assert_eq!((s.stage,s.ap_count,m.starts,m.regs[0]),(5,0,11,0));}
    #[test]fn timer_failure_stops_rx(){let mut m=fixture();m.delay_ok=false;let mut s=WifiScanResult::ZERO;assert_eq!(passive_scan(&mut m,Region::new(0x20000,0x30000).unwrap(),&mut s),Err(Error::Timer));assert_eq!(m.regs[0],0);}
}
