//! Hardware-independent WPA2-PSK connection orchestration.
//! Caller supplies fresh unpredictable SNonce and verified, leased hardware.
//! On failure caller must stop hardware and retain DMA until isolated.
use wifi_core::{frame,handshake::{Wpa2Handshake,HandshakeStep},wpa};
#[repr(C)]
#[derive(Copy, Clone)]
pub struct WifiConnectReq {
    pub ssid_len: u8,
    pub pass_len: u8,
    pub status: u8,
    pub channel: u8,
    pub bssid: [u8; 6],
    pub sta_mac: [u8; 6],
    pub ssid: [u8; 32],
    pub passphrase: [u8; 64],
    pub reserved: [u8; 48],
}

const _: () = {
    assert!(core::mem::size_of::<WifiConnectReq>() == 160);
};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum Error{Invalid,Authentication,Association,Handshake,Transport,KeyInstall}
pub trait LinkIo{
    /// Send a management or unencrypted EAPOL frame and report ACK.
    fn send(&mut self,frame:&[u8])->bool;
    /// Receive one FCS-stripped frame; None is timeout, Err is I/O fault.
    fn receive(&mut self,dst:&mut [u8],timeout_ms:u32)->Result<Option<usize>,Error>;
    fn set_aid(&mut self,aid:u16)->bool;
    fn install_keys(&mut self,tk:&[u8;16],gtk:&[u8;16],index:u8)->bool;
}
pub struct Connected{pub aid:u16,pub sequence:u16,pub handshake:Wpa2Handshake,pub tk:[u8;16]}
fn response(io:&mut impl LinkIo,buf:&mut [u8],sta:&[u8;6],ap:&[u8;6],auth:bool)->Result<Option<u16>,Error>{
    // Bound frame count as well as per-call timeout to tolerate a busy AP.
    for _ in 0..40{
        let Some(n)=io.receive(buf,10)? else{continue;};
        if n>buf.len(){return Err(Error::Transport);}
        let f=&buf[..n];
        let Some((h,_))=frame::parse_header(f)else{continue;};
        if h.addr1!=*sta||h.addr2!=*ap||h.addr3!=*ap||h.protected||h.to_ds||h.from_ds{continue;}
        if auth{
            if let Some((seq,status))=frame::parse_auth_resp(f){if seq==2&&status==0{return Ok(Some(0));}}
        }else if let Some((status,aid))=frame::parse_assoc_resp(f){
            if status==0&&aid&0x3fff!=0&&aid&0x3fff<=2007{return Ok(Some(aid&0x3fff));}
        }
    }
    Ok(None)
}
pub fn connect(io:&mut impl LinkIo,sta:[u8;6],ap:[u8;6],ssid:&[u8],pass:&[u8],snonce:[u8;32])->Result<Connected,Error>{
    if ssid.is_empty()||ssid.len()>32||!(8..=63).contains(&pass.len())||snonce==[0;32]||
       sta==[0;6]||ap==[0;6]||sta[0]&1!=0||ap[0]&1!=0{return Err(Error::Invalid);}
    let mut tx=[0u8;256];let mut rx=[0u8;1600];let mut seq=1u16;
    let mut authenticated=false;
    for _ in 0..3{
        let n=frame::build_auth(&mut tx,&sta,&ap,seq);seq=(seq+1)&0xfff;
        if n==0||!io.send(&tx[..n]){continue;}
        if response(io,&mut rx,&sta,&ap,true)?.is_some(){authenticated=true;break;}
    }
    if !authenticated{return Err(Error::Authentication);}
    let mut aid=None;
    for _ in 0..3{
        let n=frame::build_assoc_req_rsn(&mut tx,&sta,&ap,ssid,seq,true);seq=(seq+1)&0xfff;
        if n==0||!io.send(&tx[..n]){continue;}
        aid=response(io,&mut rx,&sta,&ap,false)?;
        if aid.is_some(){break;}
    }
    let aid=aid.ok_or(Error::Association)?;
    if !io.set_aid(aid){return Err(Error::Transport);}
    let pmk=wpa::derive_pmk(pass,ssid);
    let mut hs=Wpa2Handshake::new(pmk,sta,ap,snonce);
    let mut eth=[0u8;1600];let mut reply=[0u8;256];let mut outgoing=[0u8;270];let mut dot11=[0u8;320];
    for _ in 0..400{
        let Some(n)=io.receive(&mut rx,10)?else{continue;};
        if n>rx.len(){return Err(Error::Transport);}
        let Some((h,_))=frame::parse_header(&rx[..n])else{continue;};
        if h.addr1!=sta||h.addr2!=ap||!h.from_ds||h.to_ds||h.protected{continue;}
        let Some(n)=frame::decap_80211_to_eth(&mut eth,&rx[..n],&ap)else{continue;};
        if n<18||eth[..6]!=sta||eth[6..12]!=ap||eth[12..14]!=[0x88,0x8e]{continue;}
        let step=hs.process_eapol(&eth[14..n],&mut reply);
        let len=match step{
            HandshakeStep::Ignore=>continue,
            HandshakeStep::SendM2(n)=>n,
            HandshakeStep::SendM4{len,..}=>len,
            _=>return Err(Error::Handshake),
        };
        if len>reply.len(){return Err(Error::Handshake);}
        outgoing[..6].copy_from_slice(&ap);outgoing[6..12].copy_from_slice(&sta);
        outgoing[12..14].copy_from_slice(&[0x88,0x8e]);outgoing[14..14+len].copy_from_slice(&reply[..len]);
        let n=frame::encap_eth_to_80211(&mut dot11,&outgoing[..14+len],&ap,&sta,seq,None);
        seq=(seq+1)&0xfff;
        if n==0{return Err(Error::Handshake);}
        let mut ack=false;
        for _ in 0..3{if io.send(&dot11[..n]){ack=true;break;}}
        if !ack{return Err(Error::Transport);}
        if let HandshakeStep::SendM4{tk,gtk,gtk_idx,..}=step{
            if !(1..=3).contains(&gtk_idx)||!io.install_keys(&tk,&gtk,gtk_idx){return Err(Error::KeyInstall);}
            return Ok(Connected{aid,sequence:seq,handshake:hs,tk});
        }
    }
    Err(Error::Handshake)
}
#[cfg(test)]
mod tests{
    use super::*;
    struct Mock{tx:u32,rx:u32}
    impl LinkIo for Mock{
        fn send(&mut self,_:&[u8])->bool{self.tx+=1;true}
        fn receive(&mut self,_:&mut [u8],_:u32)->Result<Option<usize>,Error>{self.rx+=1;Ok(None)}
        fn set_aid(&mut self,_:u16)->bool{panic!()}
        fn install_keys(&mut self,_:&[u8;16],_:&[u8;16],_:u8)->bool{panic!()}
    }
    #[test]fn no_ap_has_bounded_retries(){let mut m=Mock{tx:0,rx:0};assert!(matches!(connect(&mut m,[2,1,2,3,4,5],[2,6,7,8,9,10],b"test",b"password",[1;32]),Err(Error::Authentication)));assert_eq!((m.tx,m.rx),(3,120));}
    #[test]fn reject_invalid_credentials_before_tx(){let mut m=Mock{tx:0,rx:0};assert!(matches!(connect(&mut m,[2,1,2,3,4,5],[2,6,7,8,9,10],b"test",b"short",[1;32]),Err(Error::Invalid)));assert_eq!(m.tx,0);}
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum Rekey{Ignore,Reply(usize)}
/// Verify and unwrap the group key using the existing handshake replay floor.
/// Install only new keys; caller must terminate the session on send failure.
pub fn group_rekey(c:&mut Connected,eapol:&[u8],reply:&mut [u8],mut install:impl FnMut(&[u8;16],u8)->bool)->Result<Rekey,Error>{
    match c.handshake.process_eapol(eapol,reply){
        HandshakeStep::Ignore=>Ok(Rekey::Ignore),
        HandshakeStep::RotateGtk{gtk,gtk_idx,reply_len}=>{
            if !(1..=3).contains(&gtk_idx)||reply_len>reply.len()||!install(&gtk,gtk_idx){return Err(Error::KeyInstall);}
            Ok(Rekey::Reply(reply_len))
        }
        _=>Err(Error::Handshake)
    }
}
