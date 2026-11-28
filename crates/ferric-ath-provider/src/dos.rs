//! DOS transaction boundary. Caller retains its allocation on status 7,
//! or on successful connection, until explicit shutdown succeeds.
use crate::{state,ath_scan,ath_connect_internal};
use ferric_ath5k::{connection::WifiConnectReq,scan::WifiScanResult,dma::WIFI_DMA_SIZE};

pub fn valid_dma_segment(seg:u32)->bool{
    (0x50..=0xffff).contains(&seg)&&(seg<<4)+WIFI_DMA_SIZE as u32<=0xa0000
}
fn buffer(seg:u32,off:u32,len:u32)->Option<u32>{
    if seg>0xffff||off>0xffff||len==0||off.checked_add(len)?>0x10000{return None;}
    let l=(seg<<4)+off;
    if l<0x500||l.checked_add(len)?>0xa0000{return None;}
    Some(l)
}
fn overlap(a:u32,n:u32,b:u32,m:u32)->bool{a<b+m&&b<a+n}
#[unsafe(no_mangle)]
pub extern "C" fn ath_dos_scan(dma:u32,seg:u32,off:u32)->u32{
    if !valid_dma_segment(dma){return 1;}
    let n=core::mem::size_of::<WifiScanResult>()as u32;
    let Some(dst)=buffer(seg,off,n)else{return 1;};
    if overlap(dst,n,dma<<4,WIFI_DMA_SIZE as u32){return 1;}
    if !state().dma.can_unload(){return 2;}
    state().scan=WifiScanResult::ZERO;
    let status=ath_scan(dma);
    unsafe{core::ptr::write_unaligned(dst as *mut WifiScanResult,state().scan)};
    status
}
/// Combined 192-byte request: existing 160-byte ABI followed by 32 random
/// bytes supplied from a trusted external generator. Wipe credentials in
/// caller memory on every accepted buffer, including a gated attempt.
#[unsafe(no_mangle)]
pub extern "C" fn ath_dos_connect(dma:u32,seg:u32,off:u32)->u32{
    if !valid_dma_segment(dma){return 1;}
    let Some(src)=buffer(seg,off,192)else{return 1;};
    if overlap(src,192,dma<<4,WIFI_DMA_SIZE as u32){return 1;}
    if let Some(r)=state().dma.region(){if overlap(src,192,r.linear,WIFI_DMA_SIZE as u32){return 1;}}
    let mut req=unsafe{core::ptr::read_unaligned(src as *const WifiConnectReq)};
    let mut nonce=unsafe{core::ptr::read_unaligned((src+160)as *const [u8;32])};
    for i in 0..192{unsafe{core::ptr::write_volatile((src+i)as *mut u8,0)}}
    let status=unsafe{ath_connect_internal(dma,&mut req,&nonce)};
    for b in &mut req.passphrase{unsafe{core::ptr::write_volatile(b,0)}}
    for b in &mut nonce{unsafe{core::ptr::write_volatile(b,0)}}
    status
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn conventional_and_nonwrapping(){
        assert!(valid_dma_segment(0x2000));assert!(!valid_dma_segment(0));assert!(!valid_dma_segment(0x9c01));
        assert_eq!(buffer(0x2000,0xff80,192),None);assert_eq!(buffer(0xa000,0,1),None);
        assert!(overlap(0x20000,192,0x20000,16384));assert!(!overlap(0x24000,192,0x20000,16384));
    }
}
