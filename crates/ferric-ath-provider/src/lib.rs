//! Experimental standalone AR2413 resident consumer. Probe only: no DMA,
//! association or packet interface until the activation and drain gates pass.
#![cfg_attr(not(test),no_std)]
#![deny(warnings)]
use core::cell::UnsafeCell;
use ferric_cardbus::CardBusSlotInfo;
use ferric_ath5k::dma_lease::Lease;
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
struct Resident{slot:Option<u32>,linear:u32,dma:Lease}
struct Cell(UnsafeCell<Resident>);
unsafe impl Sync for Cell{}
static STATE:Cell=Cell(UnsafeCell::new(Resident{slot:None,linear:0,dma:Lease::new()}));
fn state()->&'static mut Resident{unsafe{&mut *STATE.0.get()}}
fn eligible(info:&CardBusSlotInfo)->bool{
    info.present!=0&&info.stage==5&&info.card_present!=0&&info.is_cardbus!=0&&info.powered_3v3!=0&&
    info.card_vid==0x168c&&info.card_did==0x001a&&info.card_bar0!=0&&info.card_bar0&0xffff==0
}
#[unsafe(no_mangle)]
pub extern "C" fn ath_version()->u32{0x10000}
/// Status 0=probe-ready mapping, 1=invalid, 2=busy, 5=no ready child,
/// 6=mapping failed, 7=cleanup retained. No radio access or DMA.
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
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn ath_shutdown()->u32{
    let s=state();
    if !s.dma.can_unload(){return 7;}
    let Some(slot)=s.slot else{return 0;};
    if unsafe{ath_cb_master(slot,0)}!=0{return 7;}
    if s.linear!=0{
        if unsafe{ath_unmap(s.linear)}==0{return 7;}
        s.linear=0;
    }
    if unsafe{ath_cb_release(slot)}!=0{return 7;}
    s.slot=None;0
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn rejects_discovery_only(){assert!(!eligible(&CardBusSlotInfo::ZERO));assert_eq!(ath_probe(2),1);assert_eq!(ath_probe(0),5);assert_eq!(ath_shutdown(),0);}
    #[test]fn requires_exact_powered_child(){let mut i=CardBusSlotInfo::ZERO;i.present=1;i.stage=5;i.card_present=1;i.is_cardbus=1;i.powered_3v3=1;i.card_vid=0x168c;i.card_did=0x001a;i.card_bar0=0xf7d00000;assert!(eligible(&i));i.card_did=0x001b;assert!(!eligible(&i));}
}
