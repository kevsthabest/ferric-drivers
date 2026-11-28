use ferric_ath5k::registers::RegisterIo;
pub struct Mmio(pub u32);
impl RegisterIo for Mmio{
    fn read(&mut self,o:usize)->u32{
        if self.0==0||o>0xfffc||o&3!=0{return u32::MAX;}
        unsafe{core::ptr::read_volatile((self.0 as usize+o)as *const u32)}
    }
    fn write(&mut self,o:usize,v:u32){
        if self.0!=0&&o<=0xfffc&&o&3==0{unsafe{core::ptr::write_volatile((self.0 as usize+o)as *mut u32,v)}}
    }
    fn delay_ms(&mut self,ms:u32)->bool{delay(ms)}
}
#[cfg(target_arch="x86")]
fn delay(ms:u32)->bool{
    // PIT channel 0 mode 3 counts down by two. Reject implausibly large
    // deltas at the mode-3 reload boundary; undercount is safe.
    fn sample()->u16{let lo:u8;let hi:u8;unsafe{core::arch::asm!("out 0x43, al","in al, 0x40",inout("al") 0u8=>lo,options(nomem,nostack));core::arch::asm!("in al, 0x40",out("al") hi,options(nomem,nostack));}u16::from(lo)|(u16::from(hi)<<8)}
    let target=ms.saturating_mul(2387);let mut elapsed=0u32;let mut prev=sample();
    for _ in 0..ms.saturating_mul(200_000).max(200_000){
        let now=sample();let delta=prev.wrapping_sub(now);
        if delta<0x8000{elapsed=elapsed.saturating_add(u32::from(delta));}
        if elapsed>=target{return true;}prev=now;
    }
    false
}
#[cfg(not(target_arch="x86"))]
fn delay(_:u32)->bool{false}
