//! Resident VDMAD adapter. The PCI-drain qualification flag is deliberately
//! false until real ToPIC95 teardown has been validated.
use ferric_ath5k::{dma::{Region,initialize_rx,WIFI_DMA_SIZE},dma_lease::DmaPlatform,engine,radio,registers::RegisterIo};
use crate::{ath_cb_master,registers::Mmio};
pub struct Platform{pub slot:u32,pub mmio:Mmio,pub fail:u32}
const DRAIN_QUALIFIED:bool=false;
impl DmaPlatform for Platform{
    fn lock(&mut self,l:u32,n:u32)->Option<u32>{lock(l,n)}
    fn unlock(&mut self,l:u32,n:u32)->bool{unlock(l,n)}
    fn initialize(&mut self,r:Region)->bool{
        if !engine::mask_irqs(&mut self.mmio)||!engine::stop_all(&mut self.mmio).is_ok(){return false;}
        if !radio::ar2413_radio_init_11g(&mut self.mmio,2412,&mut self.fail){return false;}
        if unsafe{ath_cb_master(self.slot,0)}!=0||!engine::mask_irqs(&mut self.mmio){return false;}
        unsafe{core::ptr::write_bytes(r.linear as *mut u8,0,WIFI_DMA_SIZE);}
        initialize_rx(r,|o,v|unsafe{core::ptr::write_volatile((r.linear as usize+o)as *mut u32,v)});
        true
    }
    fn enable_master(&mut self)->bool{DRAIN_QUALIFIED&&unsafe{ath_cb_master(self.slot,1)}==0}
    fn stop_engines(&mut self)->bool{
        let masked=engine::mask_irqs(&mut self.mmio);
        let stopped=engine::stop_all(&mut self.mmio).is_ok();
        masked&&stopped
    }
    fn isolate(&mut self)->bool{
        // No claim of a qualified drain from command readback alone.
        let off=unsafe{ath_cb_master(self.slot,0)}==0;
        off&&DRAIN_QUALIFIED&&self.mmio.delay_ms(10)
    }
}
#[cfg(target_arch="x86")]
fn lock(l:u32,n:u32)->Option<u32>{
    let phys:u32;let failed:u32;
    unsafe{core::arch::asm!("push esi","mov esi,eax","int 0x20",".word 1",".word 4","sbb eax,eax","pop esi",inlateout("eax")l=>failed,inlateout("ecx")n=>_,inlateout("edx")0u32=>phys);}
    if failed==0{Some(phys)}else{None}
}
#[cfg(target_arch="x86")]
fn unlock(l:u32,n:u32)->bool{
    let failed:u32;
    unsafe{core::arch::asm!("push esi","mov esi,eax","int 0x20",".word 2",".word 4","sbb eax,eax","pop esi",inlateout("eax")l=>failed,inlateout("ecx")n=>_,inlateout("edx")0u32=>_);}
    failed==0
}
#[cfg(not(target_arch="x86"))]
fn lock(_:u32,_:u32)->Option<u32>{None}
#[cfg(not(target_arch="x86"))]
fn unlock(_:u32,_:u32)->bool{false}

pub struct ScanAdapter<'a>{pub mmio:&'a mut Mmio,pub region:Region}
impl RegisterIo for ScanAdapter<'_>{
    fn read(&mut self,o:usize)->u32{self.mmio.read(o)}
    fn write(&mut self,o:usize,v:u32){self.mmio.write(o,v)}
    fn delay_ms(&mut self,ms:u32)->bool{self.mmio.delay_ms(ms)}
}
impl ferric_ath5k::scan::ScanIo for ScanAdapter<'_>{
    fn dma_read(&mut self,o:usize)->u32{
        if o>WIFI_DMA_SIZE-4||o&3!=0{return 0;}
        unsafe{core::ptr::read_volatile((self.region.linear as usize+o)as *const u32)}
    }
    fn dma_write(&mut self,o:usize,v:u32){
        if o<=WIFI_DMA_SIZE-4&&o&3==0{unsafe{core::ptr::write_volatile((self.region.linear as usize+o)as *mut u32,v)}}
    }
    fn copy_frame(&mut self,o:usize,dst:&mut [u8])->bool{
        if o.checked_add(dst.len()).is_none_or(|end|end>WIFI_DMA_SIZE){return false;}
        for (i,v) in dst.iter_mut().enumerate(){*v=unsafe{core::ptr::read_volatile((self.region.linear as usize+o+i)as *const u8)};}
        true
    }
}
