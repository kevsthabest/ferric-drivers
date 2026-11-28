//! Resident CardBus provider. Discovery only until the platform-specific
//! firmware resource audit and physical activation gate are implemented.
#![cfg_attr(not(test), no_std)]
#![deny(warnings)]
use ferric_cardbus::{CardBusSlotInfo, TOPIC95_DID, TOPIC95_VID, TOPIC95_PCI_DEV};
use ferric_hw::pci::read32;
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
    let info=discover(slot,|f,o| unsafe { read32(0,TOPIC95_PCI_DEV,f,o) });
    unsafe { core::ptr::copy_nonoverlapping((&info as *const CardBusSlotInfo).cast::<u8>(),dst,40) };
    1
}
/// Activation deliberately fails closed until resource discovery is qualified.
#[unsafe(no_mangle)]
pub extern "C" fn cb_acquire(_slot:u32,_consumer:u32)->u32 { 5 }
#[unsafe(no_mangle)]
pub extern "C" fn cb_release(_slot:u32,_consumer:u32)->u32 { 4 }
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
