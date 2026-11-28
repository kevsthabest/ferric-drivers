//! Socket power sequencing for a claimed, mapped ToPIC95 socket.
//! Platform adapter must guarantee the requested minimum delay or fail.
use crate::{decode_socket_state, CB_BADVCCREQ, CB_PWRCYCLE, CB_CVSTEST, CB_SC_VCC_3V};

pub trait SocketIo {
    fn state(&mut self) -> u32;
    fn control(&mut self) -> u32;
    fn set_control(&mut self, v: u32);
    fn set_reset(&mut self, asserted: bool) -> bool;
    fn mask_events(&mut self);
    fn clear_events(&mut self);
    fn disable_exca_power(&mut self);
    fn force(&mut self, v: u32);
    fn delay_ms(&mut self, ms: u32) -> bool;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error { Absent, Empty, Unsupported, Voltage, Timer, Reset, Timeout, Cleanup }
fn delay(io: &mut impl SocketIo, ms: u32) -> Result<(), Error> {
    if io.delay_ms(ms) { Ok(()) } else { Err(Error::Timer) }
}
/// Caller must stop child DMA and detach consumers before invoking this.
pub fn power_off(io: &mut impl SocketIo) -> Result<(), Error> {
    if io.state() == u32::MAX { return Err(Error::Absent); }
    let reset = io.set_reset(true);
    io.mask_events();
    io.disable_exca_power();
    io.set_control(0);
    io.clear_events();
    delay(io, 250)?;
    if !reset || io.control() != 0 { return Err(Error::Cleanup); }
    Ok(())
}
fn start(io: &mut impl SocketIo) -> Result<u32, Error> {
    if io.state() == u32::MAX || io.control() == u32::MAX { return Err(Error::Absent); }
    power_off(io)?;
    if !io.set_reset(false) { return Err(Error::Reset); }
    let mut sensed = false;
    for _ in 0..2 {
        io.force(CB_CVSTEST);
        for _ in 0..10 {
            delay(io, 25)?;
            let s = io.state();
            if s == u32::MAX { return Err(Error::Absent); }
            let d = decode_socket_state(s);
            if !d.card_present { return Err(Error::Empty); }
            if d.supports_3v3 || d.supports_5v {
                if !d.is_cardbus || !d.supports_3v3 { return Err(Error::Unsupported); }
                sensed = true; break;
            }
        }
        if sensed { break; }
    }
    if !sensed { return Err(Error::Timeout); }
    if !io.set_reset(true) { return Err(Error::Reset); }
    io.set_control(CB_SC_VCC_3V);
    let mut stable = false;
    for _ in 0..30 {
        delay(io, 10)?;
        let s = io.state();
        if s == u32::MAX { return Err(Error::Absent); }
        let d = decode_socket_state(s);
        if !d.card_present { return Err(Error::Empty); }
        if s & CB_BADVCCREQ != 0 { return Err(Error::Voltage); }
        if s & CB_PWRCYCLE != 0 { stable = true; break; }
    }
    if !stable { return Err(Error::Timeout); }
    delay(io, 100)?;
    if !io.set_reset(false) { return Err(Error::Reset); }
    delay(io, 100)?;
    let s = io.state();
    if s == u32::MAX { return Err(Error::Absent); }
    if !decode_socket_state(s).card_present { return Err(Error::Empty); }
    // PWRCYCLE was observed before reset release; the captured DynaBook
    // successful state 0x30000820 no longer has it set. Require voltage
    // selection readback and a still-present 3.3 V CardBus card instead.
    let d=decode_socket_state(s);
    if s & CB_BADVCCREQ != 0 || !d.is_cardbus || !d.supports_3v3 || io.control()!=CB_SC_VCC_3V { return Err(Error::Voltage); }
    Ok(s)
}
/// On every failed startup attempt, leave power off and reset asserted.
/// Cleanup failure takes precedence; retain resources for manual recovery.
pub fn power_on(io: &mut impl SocketIo) -> Result<u32, Error> {
    match start(io) {
        Ok(s) => Ok(s),
        Err(e) => if power_off(io).is_ok() { Err(e) } else { Err(Error::Cleanup) },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CB_CBCARD, CB_3VCARD, CB_5VCARD, CB_CDETECT1};
    struct Mock { status: u32, ctrl: u32, reset: bool, fail_delay: bool, ever_powered: bool, power_status: u32, after_reset: Option<u32> }
    impl SocketIo for Mock {
        fn state(&mut self)->u32 { if self.ctrl != 0 { if !self.reset { self.after_reset.unwrap_or(self.power_status) } else {self.power_status} } else { self.status } }
        fn control(&mut self)->u32 { self.ctrl }
        fn set_control(&mut self,v:u32) { if v!=0 { assert!(self.reset); self.ever_powered=true; } self.ctrl=v; }
        fn set_reset(&mut self,v:bool)->bool { self.reset=v; true }
        fn mask_events(&mut self) {}
        fn clear_events(&mut self) {}
        fn disable_exca_power(&mut self) {}
        fn force(&mut self,_:u32) {}
        fn delay_ms(&mut self,_:u32)->bool { !self.fail_delay }
    }
    fn fixture()->Mock { Mock { status:CB_CBCARD|CB_3VCARD,ctrl:0,reset:false,fail_delay:false,ever_powered:false,power_status:CB_CBCARD|CB_3VCARD|CB_PWRCYCLE,after_reset:None } }
    #[test]
    fn successful_cycle() {
        let mut m=fixture(); assert!(power_on(&mut m).is_ok());
        assert_eq!(m.ctrl,CB_SC_VCC_3V); assert!(!m.reset);
        power_off(&mut m).unwrap(); assert_eq!(m.ctrl,0); assert!(m.reset);
    }
    #[test]
    fn power_cycle_bit_can_clear_after_reset_release() {
        let mut m=fixture();m.after_reset=Some(0x30000820);
        assert_eq!(power_on(&mut m),Ok(0x30000820));
    }
    #[test]
    fn empty_or_wrong_voltage_never_powers() {
        for status in [CB_CDETECT1, CB_5VCARD, CB_CBCARD|CB_5VCARD] {
            let mut m=fixture(); m.status=status;
            assert!(power_on(&mut m).is_err()); assert!(!m.ever_powered);
        }
    }
    #[test]
    fn power_fault_and_removal_roll_back() {
        for status in [CB_BADVCCREQ, CB_CDETECT1, CB_CBCARD|CB_3VCARD] {
            let mut m=fixture(); m.power_status=status;
            assert!(power_on(&mut m).is_err()); assert_eq!(m.ctrl,0); assert!(m.reset);
        }
    }
    #[test]
    fn failed_timer_retains_cleanup_error() {
        let mut m=fixture(); m.fail_delay=true;
        assert_eq!(power_on(&mut m),Err(Error::Cleanup)); assert!(!m.ever_powered);
    }
}
