//! Standalone packet API; no Intel hardware dependencies.
//! All calls serialized by the Jemm bridge; callbacks run outside Rust.
use core::cell::UnsafeCell;
use crate::{ath_send_internal,ath_receive_internal,state};
const EDI:usize=0;const ESI:usize=1;const EBX:usize=4;const EDX:usize=5;const ECX:usize=6;const EAX:usize=7;const CS:usize=10;const FLAGS:usize=12;const ES:usize=15;const DS:usize=16;
#[derive(Clone,Copy)]
struct Registration{active:bool,filter:Option<u16>,receiver:u32}
const EMPTY:Registration=Registration{active:false,filter:None,receiver:0};
struct Packet{regs:[Registration;4],mode:u16,frame:[u8;1600],phase:u8,handle:usize,len:usize,dest:u32,stats:[u32;7]}
impl Packet{const fn new()->Self{Self{regs:[EMPTY;4],mode:3,frame:[0;1600],phase:0,handle:0,len:0,dest:0,stats:[0;7]}}}
struct Cell(UnsafeCell<Packet>);unsafe impl Sync for Cell{}
static PACKET:Cell=Cell(UnsafeCell::new(Packet::new()));
fn packet()->&'static mut Packet{unsafe{&mut *PACKET.0.get()}}
fn linear(seg:u32,off:u32,len:usize)->Option<u32>{
    let o=off&0xffff;let l=((seg&0xffff)<<4)+o;
    if l<0x500||len==0||o as usize+len>0x10000||l as usize+len>0xa0000{return None;}
    // Reject any overlap with the device-owned DMA allocation.
    if let Some(r)=state().dma.region(){
        if l<r.linear+16384&&r.linear<l+len as u32{return None;}
    }
    Some(l)
}
fn word(c:&mut [u32;19],i:usize,v:u32){c[i]=(c[i]&0xffff0000)|(v&0xffff);}
fn error(c:&mut [u32;19],code:u32){c[EDX]=(c[EDX]&!0xff00)|(code<<8);c[FLAGS]|=1;}
fn copy_out(dst:u32,src:&[u8]){for (i,b) in src.iter().enumerate(){unsafe{core::ptr::write_volatile((dst as usize+i)as *mut u8,*b)}}}
fn active(p:&Packet,h:usize)->bool{h>=1&&h<=4&&p.regs[h-1].active}
#[unsafe(no_mangle)]
pub extern "C" fn ath_packet_busy()->u32{u32::from(packet().phase!=0||packet().regs.iter().any(|r|r.active))}
#[unsafe(no_mangle)]
pub extern "C" fn ath_packet_ready()->u32{u32::from(state().link.is_some())}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ath_packet_api(client:*mut u32)->u32{
    let c=unsafe{&mut *client.cast::<[u32;19]>()};let p=packet();
    let func=(c[EAX]>>8)&255;let al=c[EAX]&255;let h=(c[EBX]&65535)as usize;
    c[FLAGS]&=!1;
    // Permit release even after link loss so the driver can unload.
    if state().link.is_none()&&func!=3{error(c,11);return 1;}
    match func{
        1 if al==255=>{c[EAX]=(c[EAX]&!255)|2;word(c,EBX,0x101);word(c,ECX,0x100);word(c,EDX,1);let cs=c[CS];word(c,DS,cs);word(c,ESI,0x20);}
        2=>{
            if al!=1{error(c,2);return 1;}
            if c[EDX]&255!=0{error(c,4);return 1;}
            let filter=match c[ECX]&65535{0=>None,2=>{
                let Some(l)=linear(c[DS],c[ESI],2)else{error(c,5);return 1;};
                Some(u16::from_be_bytes([unsafe{core::ptr::read_volatile(l as *const u8)},unsafe{core::ptr::read_volatile((l+1)as *const u8)}]))
            },_=>{error(c,5);return 1;}};
            if linear(c[ES],c[EDI],1).is_none(){error(c,5);return 1;}
            if p.regs.iter().any(|r|r.active&&(r.filter==filter||r.filter.is_none()||filter.is_none())){error(c,10);return 1;}
            let Some(i)=p.regs.iter().position(|r|!r.active)else{error(c,9);return 1;};
            p.regs[i]=Registration{active:true,filter,receiver:((c[ES]&65535)<<16)|(c[EDI]&65535)};word(c,EAX,(i+1)as u32);
        }
        3=>{if !active(p,h){error(c,1);}else if p.phase!=0&&p.handle==h-1{error(c,13);}else{p.regs[h-1]=EMPTY;}}
        4=>{
            let len=(c[ECX]&65535)as usize;
            let Some(l)=linear(c[DS],c[ESI],len)else{error(c,12);return 1;};
            if !(14..=1514).contains(&len)||unsafe{ath_send_internal(l as *const u8,len as u32)}!=0{p.stats[5]=p.stats[5].wrapping_add(1);error(c,12);}
            else{p.stats[1]=p.stats[1].wrapping_add(1);p.stats[3]=p.stats[3].wrapping_add(len as u32);}
        }
        5=>error(c,7),
        6=>{if h!=0&&!active(p,h){error(c,1);return 1;}if c[ECX]&65535<6{error(c,9);return 1;}let Some(l)=linear(c[ES],c[EDI],6)else{error(c,9);return 1;};copy_out(l,&state().info.mac);word(c,ECX,6);}
        7=>{if !active(p,h){error(c,1);}} // no reset of live leased descriptors
        0x0a=>{let Some(l)=linear(c[DS],c[ESI],14)else{error(c,9);return 1;};copy_out(l,&[1,1,14,6,0xdc,5,0,0,8,0,1,0,0x60,0]);}
        0x14=>{if !active(p,h){error(c,1);}else if !(1..=3).contains(&(c[ECX]&65535)){error(c,8);}else{p.mode=(c[ECX]&65535)as u16;}}
        0x15=>{if !active(p,h){error(c,1);}else{word(c,EAX,u32::from(p.mode));}}
        0x18=>{if !active(p,h){error(c,1);return 1;}let Some(l)=linear(c[DS],c[ESI],28)else{error(c,9);return 1;};for(i,v)in p.stats.iter().enumerate(){copy_out(l+(i*4)as u32,&v.to_le_bytes());}}
        _=>error(c,11)
    }
    1
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ath_upcall_next(out:*mut u32)->u32{
    let p=packet();let o=unsafe{&mut *out.cast::<[u32;8]>()};
    if p.phase==2{let r=p.regs[p.handle];*o=[r.receiver,1,p.len as u32,(p.handle+1)as u32,p.dest>>16,p.dest&65535,0,0];return 1;}
    if p.phase!=0||state().link.is_none(){return 0;}
    let n=unsafe{ath_receive_internal(p.frame.as_mut_ptr(),1600)}as usize;
    if !(14..=1514).contains(&n){return 0;}
    let et=u16::from_be_bytes([p.frame[12],p.frame[13]]);
    let allow=p.mode!=1&&(p.mode!=2||p.frame[0]&1==0)&&(p.frame[0]&1==0||p.frame[..6]==[255;6]);
    let target=p.regs.iter().position(|r|r.active&&(r.filter.is_none()||r.filter==Some(et)));
    if let Some(h)=target.filter(|_|allow){p.handle=h;p.len=n;p.phase=1;*o=[p.regs[h].receiver,0,n as u32,(h+1)as u32,0,0,0,0];1}
    else{p.stats[6]=p.stats[6].wrapping_add(1);0}
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ath_upcall_after(out:*const u32){
    let p=packet();let o=unsafe{&*out.cast::<[u32;8]>()};
    if p.phase==1{
        if let Some(l)=linear(o[4],o[5],p.len){
            copy_out(l,&p.frame[..p.len]);p.dest=((o[4]&65535)<<16)|(o[5]&65535);p.phase=2;
            p.stats[0]=p.stats[0].wrapping_add(1);p.stats[2]=p.stats[2].wrapping_add(p.len as u32);
        }else{p.stats[6]=p.stats[6].wrapping_add(1);p.phase=0;}
    }else if p.phase==2{p.phase=0;}
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn buffer_limits(){assert_eq!(linear(0,0,6),None);assert_eq!(linear(0xa000,0,6),None);assert_eq!(linear(0x1000,0xffff,2),None);assert_eq!(linear(0x1000,0,6),Some(0x10000));}
    #[test]fn handle_bounds(){let mut p=Packet::new();assert!(!active(&p,0));assert!(!active(&p,5));p.regs[0].active=true;assert!(active(&p,1));}
}

#[unsafe(no_mangle)]
pub extern "C" fn ath_stub_validate(seg:u32)->u32{
    if seg>0xffff{return 0;}
    u32::from(linear(seg,0,512).is_some())
}
