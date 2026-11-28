//! Bounded DMA engine control. Never clear a TX descriptor pointer until
//! the queue is observed disabled. These are engine indications, not a
//! substitute for disabling PCI bus mastering before freeing DMA memory.
use crate::registers::RegisterIo;
const AR5K_CR: usize = 0x0008;
const AR5K_CR_RXE: u32 = 0x0000_0004;
const AR5K_CR_RXD: u32 = 0x0000_0020;
const AR5K_QCU_TXE: usize = 0x0840;
const AR5K_QCU_TXD: usize = 0x0880;
const AR5K_QUEUE_TXDP_0: usize = 0x0800;
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum StopError { Absent, Timeout, Timer }
fn wait_clear(io:&mut impl RegisterIo,reg:usize,mask:u32,limit:u32)->Result<(),StopError>{
    for _ in 0..limit {
        let v=io.read(reg);
        if v==u32::MAX{return Err(StopError::Absent);}
        if v&mask==0{return Ok(());}
        if !io.delay_ms(1){return Err(StopError::Timer);}
    }
    Err(StopError::Timeout)
}
pub fn stop_rx(io:&mut impl RegisterIo)->Result<(),StopError>{
    if io.read(AR5K_CR)==u32::MAX{return Err(StopError::Absent);}
    io.write(AR5K_CR,AR5K_CR_RXD);
    wait_clear(io,AR5K_CR,AR5K_CR_RXE,50)
}
pub fn wait_tx_idle(io:&mut impl RegisterIo)->Result<(),StopError>{
    wait_clear(io,AR5K_QCU_TXE,1,10)
}
pub fn stop_tx(io:&mut impl RegisterIo)->Result<(),StopError>{
    if io.read(AR5K_QCU_TXE)==u32::MAX{return Err(StopError::Absent);}
    io.write(AR5K_QCU_TXD,1);
    wait_clear(io,AR5K_QCU_TXE,1,20)?;
    io.write(AR5K_QUEUE_TXDP_0,0);
    io.write(AR5K_QCU_TXD,0);
    Ok(())
}
pub fn stop_all(io:&mut impl RegisterIo)->Result<(),StopError>{
    let rx=stop_rx(io);
    let tx=stop_tx(io);
    rx.and(tx)
}
#[cfg(test)]
mod tests{
    use super::*;
    struct Mock{rx:u32,tx:u32,ptr:u32,disabled:u32,stuck:bool,timer:bool}
    impl RegisterIo for Mock{
        fn read(&mut self,o:usize)->u32{match o{AR5K_CR=>self.rx,AR5K_QCU_TXE=>self.tx,_=>0}}
        fn write(&mut self,o:usize,v:u32){match o{
            AR5K_CR=>if !self.stuck{self.rx=0},
            AR5K_QCU_TXD=>{self.disabled=v;if v==1&&!self.stuck{self.tx=0;}},
            AR5K_QUEUE_TXDP_0=>{assert_eq!(self.tx,0);self.ptr=v;},_=>{}}}
        fn delay_ms(&mut self,_:u32)->bool{self.timer}
    }
    fn fixture()->Mock{Mock{rx:4,tx:1,ptr:0x12340,disabled:0,stuck:false,timer:true}}
    #[test]fn stop_both(){let mut m=fixture();assert_eq!(stop_all(&mut m),Ok(()));assert_eq!((m.rx,m.tx,m.ptr,m.disabled),(0,0,0,0));}
    #[test]fn stuck_queue_keeps_descriptor(){let mut m=fixture();m.stuck=true;assert_eq!(stop_tx(&mut m),Err(StopError::Timeout));assert_eq!(m.ptr,0x12340);assert_eq!(m.disabled,1);}
    #[test]fn absent_and_timer(){let mut m=fixture();m.tx=u32::MAX;assert_eq!(stop_tx(&mut m),Err(StopError::Absent));m.tx=1;m.stuck=true;m.timer=false;assert_eq!(stop_all(&mut m),Err(StopError::Timer));}
}
