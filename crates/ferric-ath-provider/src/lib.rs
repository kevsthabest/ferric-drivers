//! Experimental standalone AR2413 resident consumer. Probe only: no DMA,
//! association or packet interface until the activation and drain gates pass.
#![cfg_attr(not(test),no_std)]
#![deny(warnings)]
use core::cell::UnsafeCell;
use ferric_cardbus::CardBusSlotInfo;
use ferric_ath5k::dma_lease::Lease;
use ferric_ath5k::scan::WifiScanResult;
use ferric_ath5k::probe::{WifiProbeInfo,read_revision,probe_mapped};
mod registers;
mod dma_platform;
#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo)->!{loop{}}
#[cfg(not(test))]
unsafe extern "C"{
    fn ath_cb_query(slot:u32,dst:*mut CardBusSlotInfo)->u32;
    fn ath_cb_acquire(slot:u32)->u32;
    fn ath_cb_release(slot:u32)->u32;
    fn ath_cb_master(slot:u32,enable:u32)->u32;
    fn ath_map(physical:u32,pages:u32)->u32;
    fn ath_unmap(linear:u32)->u32;
}
#[cfg(test)]
unsafe fn ath_cb_query(_:u32,_:*mut CardBusSlotInfo)->u32{0}
#[cfg(test)]
unsafe fn ath_cb_acquire(_:u32)->u32{5}
#[cfg(test)]
unsafe fn ath_cb_release(_:u32)->u32{4}
#[cfg(test)]
unsafe fn ath_cb_master(_:u32,_:u32)->u32{5}
#[cfg(test)]
unsafe fn ath_map(_:u32,_:u32)->u32{0}
#[cfg(test)]
unsafe fn ath_unmap(_:u32)->u32{0}
struct Resident{slot:Option<u32>,linear:u32,dma:Lease,info:WifiProbeInfo,scan:WifiScanResult}
struct Cell(UnsafeCell<Resident>);
unsafe impl Sync for Cell{}
static STATE:Cell=Cell(UnsafeCell::new(Resident{slot:None,linear:0,dma:Lease::new(),info:WifiProbeInfo::ZERO,scan:WifiScanResult::ZERO}));
fn state()->&'static mut Resident{unsafe{&mut *STATE.0.get()}}
fn eligible(info:&CardBusSlotInfo)->bool{
    info.present!=0&&info.stage==5&&info.card_present!=0&&info.is_cardbus!=0&&info.powered_3v3!=0&&
    info.card_vid==0x168c&&info.card_did==0x001a&&info.card_bar0!=0&&info.card_bar0&0xffff==0
}
#[unsafe(no_mangle)]
pub extern "C" fn ath_version()->u32{0x10000}
/// Status 0=MAC/EEPROM probe completed, 1=invalid, 2=busy, 5=no ready child,
/// 6=mapping failed, 7=cleanup retained, 8=partial MAC/EEPROM probe. No DMA.
#[unsafe(no_mangle)]
pub extern "C" fn ath_probe(slot:u32)->u32{
    if slot>=2{return 1;}
    let s=state();if s.slot.is_some(){return 2;}
    let mut info=CardBusSlotInfo::ZERO;
    if unsafe{ath_cb_query(slot,&mut info)}!=1||!eligible(&info){return 5;}
    if unsafe{ath_cb_acquire(slot)}!=0{return 5;}
    s.slot=Some(slot);
    if unsafe{ath_cb_master(slot,0)}!=0{return 7;}
    s.linear=unsafe{ath_map(info.card_bar0,16)};
    if s.linear==0 {
        return if ath_shutdown()==0{6}else{7};
    }
    let mut p=WifiProbeInfo::ZERO;
    p.present=1;p.stage=2;p.slot=slot as u8;p.pci_vid=info.card_vid;p.pci_did=info.card_did;
    p.bar0_phys=info.card_bar0;p.bar0_lin=s.linear;
    let mut mmio=registers::Mmio(s.linear);
    let rev=read_revision(&mut mmio);
    probe_mapped(&mut mmio,&mut p,rev);
    // PCI reset can alter configuration; recheck mastering even on failure.
    s.info=p;
    if unsafe{ath_cb_master(slot,0)}!=0{return 7;}
    if p.stage==4{0}else{8}
}
/// Experimental DMA start. Fails closed before locking any DOS memory
/// until the PCI drain gate is qualified.
#[unsafe(no_mangle)]
pub extern "C" fn ath_dma_start(seg:u32)->u32{
    let s=state();
    let Some(slot)=s.slot else{return 5;};
    if s.info.stage!=4||s.info.mac_ok==0||seg==0||seg>0xffff{return 1;}
    let mut io=dma_platform::Platform{slot,mmio:registers::Mmio(s.linear),fail:0};
    match s.dma.start(&mut io,seg<<4){Ok(_)=>0,Err(_)=>if s.dma.can_unload(){9}else{7}}
}
/// Internal scan transaction. No DOS DMA service until the allocation
/// lifetime and drain gate are qualified.
#[unsafe(no_mangle)]
pub extern "C" fn ath_scan(seg:u32)->u32{
    let status=ath_dma_start(seg);
    if status!=0{return status;}
    let s=state();
    let (Some(slot),Some(region))=(s.slot,s.dma.region())else{return 7;};
    let mut io=dma_platform::Platform{slot,mmio:registers::Mmio(s.linear),fail:0};
    let result={
        let mut scan_io=dma_platform::ScanAdapter{mmio:&mut io.mmio,region};
        ferric_ath5k::scan::passive_scan(&mut scan_io,region,&mut s.scan)
    };
    if s.dma.shutdown(&mut io).is_err(){return 7;}
    if result.is_ok(){0}else{8}
}
/// Ring-0 caller provides a writable 44-byte ABI buffer. DOS wrapper bounds it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ath_query(dst:*mut WifiProbeInfo)->u32{
    if dst.is_null(){return 1;}
    let mut p=state().info;
    p.bar0_lin=0; // do not expose privileged linear address to DOS
    unsafe{core::ptr::write_unaligned(dst,p)};
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn ath_shutdown()->u32{
    let s=state();
    let Some(slot)=s.slot else{return 0;};
    if !s.dma.can_unload(){
        let mut io=dma_platform::Platform{slot,mmio:registers::Mmio(s.linear),fail:0};
        if s.dma.shutdown(&mut io).is_err(){return 7;}
    }
    if unsafe{ath_cb_master(slot,0)}!=0{return 7;}
    if s.linear!=0{
        if unsafe{ath_unmap(s.linear)}==0{return 7;}
        s.linear=0;
    }
    if unsafe{ath_cb_release(slot)}!=0{return 7;}
    s.slot=None;s.info=WifiProbeInfo::ZERO;s.scan=WifiScanResult::ZERO;0
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn rejects_discovery_only(){assert!(!eligible(&CardBusSlotInfo::ZERO));assert_eq!(ath_probe(2),1);assert_eq!(ath_probe(0),5);assert_eq!(ath_shutdown(),0);}
    #[test]fn requires_exact_powered_child(){let mut i=CardBusSlotInfo::ZERO;i.present=1;i.stage=5;i.card_present=1;i.is_cardbus=1;i.powered_3v3=1;i.card_vid=0x168c;i.card_did=0x001a;i.card_bar0=0xf7d00000;assert!(eligible(&i));i.card_did=0x001b;assert!(!eligible(&i));}
}
