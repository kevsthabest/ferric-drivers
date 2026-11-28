//! Transaction coordinator. Platform implements verified resource audit,
//! mapping and child quiescence. A failed rollback pins the lease.
use crate::{SlotResources, config::{ConfigIo, Snapshot}, lifecycle::{Broker, Lease}, power::{self, SocketIo}, resources::Range};
pub trait Platform: Broker + ConfigIo + SocketIo {
    /// Map socket and enable bridge memory decode, never child bus mastering.
    fn open_socket(&mut self, physical: u32) -> bool;
    /// Disable decoding and unmap, retryable. Must handle partial open.
    fn close_socket(&mut self) -> bool;
    /// Validate child is quiescent before power removal.
    fn child_idle(&mut self) -> bool;
    /// Prepare and release the bridge-owned child claim, with retryable cleanup.
    fn prepare_child(&mut self) -> bool;
    fn release_child(&mut self) -> bool;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error { Busy, Resources, Config, Mapping, Power, Consumer, Cleanup }
pub struct Session {
    lease: Lease,
    resources: SlotResources,
    slot:u8,
    snapshot: Option<Snapshot>,
    socket_attempted: bool,
    socket_open: bool,
    ready: bool,
    child_attempted: bool,
}
impl Session {
    pub fn new(slot:u8,r:SlotResources,occupied:&[Range],buses:&[u8])->Result<Self,Error> {
        Ok(Self { lease:Lease::new(slot,r,occupied,buses).map_err(|_|Error::Resources)?,
            resources:r,slot,snapshot:None,socket_attempted:false,socket_open:false,ready:false,child_attempted:false })
    }
    pub fn start(&mut self,io:&mut impl Platform,rev:u8)->Result<u32,Error> {
        if self.lease.is_held() { return Err(Error::Busy); }
        self.lease.acquire(io).map_err(|_|Error::Resources)?;
        let result=self.start_claimed(io,self.resources,rev);
        match result {
            Ok(s)=>{self.ready=true;Ok(s)}
            Err(e)=>if self.stop(io).is_ok(){Err(e)}else{Err(Error::Cleanup)},
        }
    }
    fn start_claimed(&mut self,io:&mut impl Platform,r:SlotResources,rev:u8)->Result<u32,Error> {
        self.snapshot=Some(Snapshot::capture_dynabook(io,self.slot).map_err(|_|Error::Config)?);
        self.snapshot.as_ref().ok_or(Error::Config)?.stage(io,r,rev).map_err(|_|Error::Config)?;
        self.socket_attempted=true;
        if !io.open_socket(r.bar0_phys){return Err(Error::Mapping);}
        self.socket_open=true;
        let state=power::power_on(io).map_err(|_|Error::Power)?;
        self.child_attempted=true;
        if !io.prepare_child(){return Err(Error::Resources);}
        Ok(state)
    }
    pub fn attach(&mut self,id:u16)->Result<(),Error> {
        if !self.ready{return Err(Error::Consumer);}
        self.lease.attach_consumer(id).map_err(|_|Error::Consumer)
    }
    pub fn detach(&mut self,id:u16)->Result<(),Error>{
        self.lease.detach_consumer(id).map_err(|_|Error::Consumer)
    }
    pub fn stop(&mut self,io:&mut impl Platform)->Result<(),Error> {
        if self.lease.has_consumer(){return Err(Error::Consumer);}
        self.ready=false;
        if self.child_attempted {
            if !io.child_idle() || !io.release_child(){return Err(Error::Cleanup);}
            self.child_attempted=false;
        }
        if self.socket_open {
            if !io.child_idle(){return Err(Error::Cleanup);}
            power::power_off(io).map_err(|_|Error::Cleanup)?;
            self.socket_open=false;
        }
        if self.socket_attempted {
            if !io.close_socket(){return Err(Error::Cleanup);}
            self.socket_attempted=false;
        }
        if let Some(s)=&self.snapshot {
            s.restore(io).map_err(|_|Error::Cleanup)?;
            self.snapshot=None;
        }
        self.lease.release(io).map_err(|_|Error::Cleanup)
    }
    pub fn is_held(&self)->bool{self.lease.is_held()}
    pub fn consumer_is(&self,id:u16)->bool{self.ready&&self.lease.consumer_is(id)}
}

#[cfg(test)]
mod tests{
 use super::*;
 use crate::{lifecycle::Claim, CB_CBCARD,CB_3VCARD,CB_PWRCYCLE};
 struct Mock{cfg:[u32;64],claims:u8,ctrl:u32,reset:bool,mapped:bool,child:bool,fail_open:bool,fail_child:bool,fail_close:bool}
 impl Broker for Mock{
  fn claim(&mut self,_:Claim)->bool{self.claims+=1;true}
  fn release(&mut self,_:Claim)->bool{assert!(!self.mapped&&!self.child);self.claims-=1;true}
 }
 impl ConfigIo for Mock{
  fn read(&mut self,o:u8)->u32{self.cfg[o as usize/4]}
  fn write(&mut self,o:u8,v:u32){self.cfg[o as usize/4]=v;}
 }
 impl SocketIo for Mock{
  fn state(&mut self)->u32{CB_CBCARD|CB_3VCARD|CB_PWRCYCLE}
  fn control(&mut self)->u32{self.ctrl}
  fn set_control(&mut self,v:u32){self.ctrl=v;}
  fn set_reset(&mut self,v:bool)->bool{self.reset=v;true}
  fn mask_events(&mut self){}
  fn clear_events(&mut self){}
  fn disable_exca_power(&mut self){}
  fn force(&mut self,_:u32){}
  fn delay_ms(&mut self,_:u32)->bool{true}
 }
 impl Platform for Mock{
  fn open_socket(&mut self,_:u32)->bool{assert_eq!(self.claims,4);self.mapped=true;!self.fail_open}
  fn close_socket(&mut self)->bool{if self.fail_close{return false;}self.mapped=false;true}
  fn child_idle(&mut self)->bool{true}
  fn prepare_child(&mut self)->bool{self.child=true;!self.fail_child}
  fn release_child(&mut self)->bool{self.child=false;true}
 }
 fn fixture()->Mock{
  let mut m=Mock{cfg:[0;64],claims:0,ctrl:0,reset:false,mapped:false,child:false,fail_open:false,fail_child:false,fail_close:false};
  m.cfg[0]=0x06171179;m.cfg[1]=7;m.cfg[2]=0x06070032;m.cfg[6]=0x00030300;m.cfg[15]=0x1ff;m
 }
 fn session()->Session{Session::new(0,crate::slot_resources(0).unwrap(),&[],&[]).unwrap()}
 #[test]fn successful_session_and_consumer_pin(){
  let mut m=fixture();let initial=m.cfg;let mut s=session();
  assert!(s.start(&mut m,0x32).is_ok());assert!(m.mapped&&m.child);
  s.attach(0xfd0a).unwrap();assert_eq!(s.stop(&mut m),Err(Error::Consumer));
  s.detach(0xfd0a).unwrap();s.stop(&mut m).unwrap();
  assert_eq!(m.cfg,initial);assert_eq!(m.claims,0);assert!(!s.is_held());
 }
 #[test]fn partial_map_and_child_failures_restore(){
  for child in [false,true]{
   let mut m=fixture();let initial=m.cfg;m.fail_open=!child;m.fail_child=child;let mut s=session();
   assert!(s.start(&mut m,0x32).is_err());assert_eq!(m.cfg,initial);
   assert_eq!(m.claims,0);assert!(!m.mapped&&!m.child&&!s.is_held());
  }
 }
 #[test]fn failed_rollback_retains_claims_until_retry(){
  let mut m=fixture();let initial=m.cfg;m.fail_open=true;m.fail_close=true;let mut s=session();
  assert_eq!(s.start(&mut m,0x32),Err(Error::Cleanup));assert!(s.is_held());assert_eq!(m.claims,4);
  m.fail_close=false;s.stop(&mut m).unwrap();assert_eq!(m.cfg,initial);assert_eq!(m.claims,0);
 }
}
