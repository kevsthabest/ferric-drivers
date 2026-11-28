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
}
impl Session {
    pub fn new(slot:u8,r:SlotResources,occupied:&[Range],buses:&[u8])->Result<Self,Error> {
        Ok(Self { lease:Lease::new(slot,r,occupied,buses).map_err(|_|Error::Resources)?,
            resources:r,slot,snapshot:None,socket_attempted:false,socket_open:false,ready:false })
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
        power::power_on(io).map_err(|_|Error::Power)
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
}
