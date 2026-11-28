//! Explicit DMA lifetime for the standalone backend. A failed operation
//! never discards a lock or assumes a failed enable left mastering disabled.
use crate::dma::{Region,WIFI_DMA_SIZE};

pub trait DmaPlatform {
    /// Return a locked, contiguous physical range or None (no lock retained).
    fn lock(&mut self,linear:u32,len:u32)->Option<u32>;
    /// Release the exact lock, reporting failure without losing ownership.
    fn unlock(&mut self,linear:u32,len:u32)->bool;
    /// Prepare descriptor memory while mastering is disabled.
    fn initialize(&mut self,region:Region)->bool;
    /// Enable child and bridge PCI mastering, with readback.
    fn enable_master(&mut self)->bool;
    /// Mask interrupts and stop all engines, with bounded polling.
    fn stop_engines(&mut self)->bool;
    /// Disable child and bridge mastering, verify and drain outstanding DMA.
    fn isolate(&mut self)->bool;
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum Error{Busy,Invalid,Lock,Initialize,Enable,Cleanup}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum State{Empty,Locked,PossiblyActive,Isolated}
pub struct Lease{state:State,linear:u32,region:Option<Region>}
impl Default for Lease{fn default()->Self{Self::new()}}
impl Lease{
    pub const fn new()->Self{Self{state:State::Empty,linear:0,region:None}}
    pub fn state(&self)->State{self.state}
    pub fn region(&self)->Option<Region>{self.region}
    pub fn can_unload(&self)->bool{self.state==State::Empty}
    pub fn start(&mut self,io:&mut impl DmaPlatform,linear:u32)->Result<Region,Error>{
        if !self.can_unload(){return Err(Error::Busy);}
        if linear==0||linear&15!=0||linear.checked_add(WIFI_DMA_SIZE as u32).is_none_or(|end|end>0x100000){return Err(Error::Invalid);}
        // No engine can be active before descriptor initialization.
        if !io.isolate(){return Err(Error::Cleanup);}
        let physical=io.lock(linear,WIFI_DMA_SIZE as u32).ok_or(Error::Lock)?;
        self.linear=linear;self.state=State::Locked;
        let Some(region)=Region::new(linear,physical)else{
            return if self.shutdown(io).is_ok(){Err(Error::Invalid)}else{Err(Error::Cleanup)};
        };
        self.region=Some(region);
        // Initialization may reset PCI configuration or partially start an engine.
        // Treat failure as potentially active until stop and isolation are verified.
        self.state=State::PossiblyActive;
        if !io.initialize(region){
            return if self.shutdown(io).is_ok(){Err(Error::Initialize)}else{Err(Error::Cleanup)};
        }
        // Even failure may have enabled the PCI command bit.
        self.state=State::PossiblyActive;
        if !io.enable_master(){
            return if self.shutdown(io).is_ok(){Err(Error::Enable)}else{Err(Error::Cleanup)};
        }
        Ok(region)
    }
    pub fn shutdown(&mut self,io:&mut impl DmaPlatform)->Result<(),Error>{
        if self.state==State::Empty{return Ok(());}
        if self.state==State::PossiblyActive{
            let stopped=io.stop_engines();
            let isolated=io.isolate();
            if !stopped||!isolated{return Err(Error::Cleanup);}
            self.state=State::Isolated;
        }
        if !io.unlock(self.linear,WIFI_DMA_SIZE as u32){return Err(Error::Cleanup);}
        self.state=State::Empty;self.linear=0;self.region=None;
        Ok(())
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    struct Mock{lock:bool,master:bool,fail_enable:bool,fail_stop:bool,fail_isolate:bool,fail_unlock:bool,initialized:bool,fail_initialize:bool}
    impl DmaPlatform for Mock{
        fn lock(&mut self,_:u32,_:u32)->Option<u32>{assert!(!self.master);self.lock=true;Some(0x40000)}
        fn unlock(&mut self,_:u32,_:u32)->bool{assert!(!self.master);if self.fail_unlock{return false;}self.lock=false;true}
        fn initialize(&mut self,_:Region)->bool{assert!(self.lock&&!self.master);self.initialized=true;!self.fail_initialize}
        fn enable_master(&mut self)->bool{assert!(self.initialized&&self.lock);self.master=true;!self.fail_enable}
        fn stop_engines(&mut self)->bool{!self.fail_stop}
        fn isolate(&mut self)->bool{if self.fail_isolate{return false;}self.master=false;true}
    }
    fn mock()->Mock{Mock{lock:false,master:false,fail_enable:false,fail_stop:false,fail_isolate:false,fail_unlock:false,initialized:false,fail_initialize:false}}
    #[test]fn normal_and_duplicate_start(){let mut m=mock();let mut l=Lease::new();assert!(l.start(&mut m,0x20000).is_ok());assert_eq!(l.start(&mut m,0x30000),Err(Error::Busy));assert!(!l.can_unload());l.shutdown(&mut m).unwrap();assert!(l.can_unload()&&!m.lock);}
    #[test]fn failed_stop_and_isolation_keep_lock(){let mut m=mock();let mut l=Lease::new();l.start(&mut m,0x20000).unwrap();m.fail_stop=true;assert_eq!(l.shutdown(&mut m),Err(Error::Cleanup));assert!(m.lock&&!l.can_unload());m.fail_stop=false;m.fail_isolate=true;assert!(l.shutdown(&mut m).is_err());assert!(m.lock);m.fail_isolate=false;l.shutdown(&mut m).unwrap();}
    #[test]fn uncertain_enable_and_unlock_retry(){let mut m=mock();m.fail_enable=true;m.fail_unlock=true;let mut l=Lease::new();assert_eq!(l.start(&mut m,0x20000),Err(Error::Cleanup));assert_eq!(l.state(),State::Isolated);assert!(m.lock&&!m.master);m.fail_unlock=false;l.shutdown(&mut m).unwrap();assert!(l.can_unload());}
    #[test]fn uncertain_initialization_retains_lock(){
        let mut m=mock();let mut l=Lease::new();
        m.fail_initialize=true;m.fail_stop=true;
        assert_eq!(l.start(&mut m,0x20000),Err(Error::Cleanup));
        assert!(m.lock&&!l.can_unload());m.fail_stop=false;
        l.shutdown(&mut m).unwrap();assert!(!m.lock);
    }
    #[test]fn invalid_dos_range(){let mut m=mock();let mut l=Lease::new();assert_eq!(l.start(&mut m,0xff000),Err(Error::Invalid));assert!(!m.lock);}
}
