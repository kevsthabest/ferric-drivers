//! Resident CardBus provider. Discovery only until the platform-specific
//! firmware resource audit and physical activation gate are implemented.
#![cfg_attr(not(test), no_std)]
#![deny(warnings)]
use ferric_cardbus::{CardBusSlotInfo, TOPIC95_DID, TOPIC95_VID, TOPIC95_PCI_DEV};
use ferric_hw::pci::read32;
mod platform;
use platform::Hardware;
use ferric_cardbus::provider::Session;
use core::cell::UnsafeCell;
struct Active { session:Session, hw:Hardware }
struct Slots(UnsafeCell<[Option<Active>;2]>);
unsafe impl Sync for Slots {}
static SLOTS:Slots=Slots(UnsafeCell::new([None,None]));
fn slots()->&'static mut [Option<Active>;2]{unsafe{&mut *SLOTS.0.get()}}
#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

fn discover(slot: u32, mut read: impl FnMut(u8,u8)->u32) -> CardBusSlotInfo {
    let mut info = CardBusSlotInfo::ZERO;
    if slot >= 2 { return info; }
    let f=slot as u8;
    if read(f,0) != (u32::from(TOPIC95_DID)<<16 | u32::from(TOPIC95_VID)) { return info; }
    let class=read(f,8);
    let header=read(f,12);
    if class>>8 != 0x060700 || (header>>16)&0x7f != 2 { return info; }
    info.present=1;
    info.stage=1;
    info.rev=class as u8;
    let bar=read(f,0x10);
    info.bar0_phys=ferric_hw::memory_bar32(bar).unwrap_or(0);
    info.sec_bus=(read(f,0x18)>>8) as u8;
    info.mem0_base=read(f,0x1c)&!0xfff;
    info
}
#[unsafe(no_mangle)]
pub extern "C" fn cb_version()->u32 { 0x00010000 }
#[unsafe(no_mangle)]
pub extern "C" fn cb_probe_count()->u32 {
    (0..2).map(|slot| u32::from(discover(slot,|f,o| unsafe { read32(0,TOPIC95_PCI_DEV,f,o) }).present)).sum()
}
/// Ring-0 caller supplies a writable 40-byte buffer. V86 wrapper checks bounds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cb_query(slot:u32,dst:*mut u8)->u32 {
    if slot>=2 || dst.is_null() { return 0; }
    let mut info=discover(slot,|f,o| unsafe { read32(0,TOPIC95_PCI_DEV,f,o) });
    if let Some(a)=&slots()[slot as usize] {
        info.bar0_lin=a.hw.mapped();
        if info.bar0_lin!=0 {
            info.socket_state=a.hw.socket_state();
            let d=ferric_cardbus::decode_socket_state(info.socket_state);
            info.card_present=u8::from(d.card_present);
            info.is_cardbus=u8::from(d.is_cardbus);
            info.powered_3v3=u8::from(a.hw.socket_control()==ferric_cardbus::CB_SC_VCC_3V);
            if let Some(child)=a.hw.child(){
                if d.card_present && d.is_cardbus && info.powered_3v3!=0 {
                    info.stage=5;info.card_vid=child.vendor;info.card_did=child.device;
                    info.card_rev=child.revision;info.card_subsys_vid=child.subsystem as u16;
                    info.card_subsys_did=(child.subsystem>>16) as u16;
                    info.card_class=0x020000;info.card_bar0=child.bar;
                }
            }
        }
    }
    unsafe { core::ptr::copy_nonoverlapping((&info as *const CardBusSlotInfo).cast::<u8>(),dst,40) };
    1
}
/// Activation deliberately fails closed until resource discovery is qualified.
#[unsafe(no_mangle)]
pub extern "C" fn cb_acquire(slot:u32,consumer:u32)->u32 {
    if slot>=2 || consumer==0 || consumer>0xffff{return 1;}
    let Some(a)=slots()[slot as usize].as_mut() else{return 5;};
    if consumer!=0xfd0a || a.hw.child().is_none(){return 5;}
    let d=ferric_cardbus::decode_socket_state(a.hw.socket_state());
    if !d.card_present || !d.is_cardbus{return 5;}
    if a.session.attach(consumer as u16).is_ok(){0}else{2}
}
#[unsafe(no_mangle)]
pub extern "C" fn cb_release(slot:u32,consumer:u32)->u32 {
    if slot>=2 || consumer==0 || consumer>0xffff{return 1;}
    let Some(a)=slots()[slot as usize].as_mut() else{return 4;};
    if !a.session.consumer_is(consumer as u16){return 4;}
    if !a.hw.set_dma_master(false){return 2;}
    if a.session.detach(consumer as u16).is_ok(){0}else{4}
}
/// Ring-0 consumer only. Caller must lock and initialize descriptors before
/// enabling, and verify engine stop/drain before releasing DMA memory.
#[unsafe(no_mangle)]
pub extern "C" fn cb_dma_master(slot:u32,consumer:u32,enable:u32)->u32{
    if slot>=2||consumer!=0xfd0a||enable>1{return 1;}
    let Some(a)=slots()[slot as usize].as_mut()else{return 5;};
    if !a.session.consumer_is(consumer as u16){return 4;}
    // Separate gate: no bus mastering until physical drain is qualified.
    if enable!=0{return 6;}
    if a.hw.set_dma_master(false){0}else{2}
}
struct AuditPci;
impl ferric_cardbus::audit::PciRead for AuditPci {
    fn read(&mut self,b:u8,d:u8,f:u8,o:u8)->u32{unsafe{read32(b,d,f,o)}}
}
#[unsafe(no_mangle)]
pub extern "C" fn cb_audit(slot:u32)->u32{
    if slot>=2{return 1;}
    let Some(r)=ferric_cardbus::slot_resources(slot as usize)else{return 1;};
    ferric_cardbus::audit::inspect(&mut AuditPci,slot as u8,r) as u32
}
/// Slot-zero gated start: 0 ready, 1 invalid, 2 busy, 5 preflight or
/// transaction failed, 6 incomplete authorization, 7 cleanup retained.
#[unsafe(no_mangle)]
pub extern "C" fn cb_start_audited(slot:u32)->u32 {
    if slot!=0{return 1;}
    if slots()[0].is_some(){return 2;}
    if cb_audit(slot)!=0{return 5;}
    if !ferric_cardbus::dynabook::matches_capture(&mut AuditPci){return 5;}
    // Read-only PCI audit does not authorize any physical address range.
    // This gate must not be bypassed by a future V86 wrapper.
    const EVIDENCE:ferric_cardbus::authorization::Evidence<'static>=ferric_cardbus::authorization::Evidence{
        root_windows:&[],occupied:&[ferric_cardbus::dynabook::AGP,ferric_cardbus::dynabook::GRAPHICS],
        occupied_buses:&[1,3,4],complete:false,
    };
    let Some(r)=ferric_cardbus::slot_resources(0)else{return 1;};
    if ferric_cardbus::authorization::check(0,r,&EVIDENCE).is_err(){return 6;}
    // A PCI-only scan is insufficient; platform firmware authorization remains absent.
    let cell=&mut slots()[slot as usize];if cell.is_some(){return 2;}
    let info=discover(slot,|f,o|unsafe{read32(0,TOPIC95_PCI_DEV,f,o)});
    if info.present==0{return 1;}
    let Ok(session)=Session::new(slot as u8,r,EVIDENCE.occupied,EVIDENCE.occupied_buses) else{return 1;};
    let mut a=Active{session,hw:Hardware::new(slot as u8,r)};
    let result=a.session.start(&mut a.hw,info.rev);
    if a.session.is_held(){*cell=Some(a);}
    if result.is_ok(){0}else if cell.is_some(){7}else{5}
}
#[unsafe(no_mangle)]
pub extern "C" fn cb_shutdown()->u32 {
    for cell in slots(){if let Some(a)=cell.as_mut(){if let Err(e)=a.session.stop(&mut a.hw){return if e==ferric_cardbus::provider::Error::Consumer{2}else{7};}*cell=None;}}
    0
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_is_read_only_and_validates_header() {
        let i=discover(0,|_,o| match o {0=>0x06171179,8=>0x06070032,12=>0x00020000,0x10=>0xf7e00000,0x18=>0x00020200,0x1c=>0xf7d00000,_=>0});
        assert_eq!(i.present,1); assert_eq!(i.stage,1); assert_eq!(i.sec_bus,2);
        assert_eq!(i.powered_3v3,0); assert_eq!(i.bar0_phys,0xf7e00000);
        assert_eq!(discover(2,|_,_| panic!()).present,0);
        assert_eq!(discover(0,|_,_| u32::MAX).present,0);
        assert_eq!(cb_acquire(0,0xfd0a),5);
    }
}
