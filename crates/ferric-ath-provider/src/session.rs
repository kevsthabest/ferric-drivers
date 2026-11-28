//! Resident WPA2 connection over the shared polled transport.
use ferric_ath5k::{connection::{self,WifiConnectReq,Connected,Error},dma::{Region,initialize_rx},engine,radio,transport::{self,Transport,TransportIo}};
pub struct Link{pub connected:Connected,pub sta:[u8;6],pub ap:[u8;6],pub rx_index:usize,pub pn:u64}
pub fn valid(r:&WifiConnectReq,n:&[u8;32])->bool{
    (1..=32).contains(&r.ssid_len)&&(8..=63).contains(&r.pass_len)&&(1..=11).contains(&r.channel)&&r.bssid!=[0;6]&&r.bssid[0]&1==0&&*n!=[0;32]
}
pub fn establish(io:&mut impl TransportIo,region:Region,sta:[u8;6],req:&WifiConnectReq,snonce:[u8;32])->Result<Link,Error>{
    if !valid(req,&snonce){return Err(Error::Invalid);}
    if !engine::mask_irqs(io)||engine::stop_all(io).is_err(){return Err(Error::Transport);}
    // DMA is already leased. Use channel tuning, not another PCI reset
    // which could invalidate the verified master/bridge configuration.
    if !radio::ar2413_set_channel_2g(io,2407+5*u16::from(req.channel)){return Err(Error::Transport);}
    if !transport::ar2413_init_tx_q0(io){return Err(Error::Transport);}
    transport::ar2413_clear_keycache(io);
    transport::ar2413_set_sta_bssid(io,&sta,&req.bssid,0);
    initialize_rx(region,|o,v|io.dma_write(o,v));
    if !engine::start_rx(io,region.physical){return Err(Error::Transport);}
    let mut t=Transport{io,region,sta,ap:req.bssid,rx_index:0};
    let connected=connection::connect(&mut t,sta,req.bssid,&req.ssid[..usize::from(req.ssid_len)],&req.passphrase[..usize::from(req.pass_len)],snonce)?;
    Ok(Link{connected,sta,ap:req.bssid,rx_index:t.rx_index,pn:1})
}
impl Link{
    pub fn send(&mut self,io:&mut impl TransportIo,region:Region,eth:&[u8])->bool{
        if !(14..=1514).contains(&eth.len())||eth[6..12]!=self.sta{return false;}
        let mut frame=[0u8;1600];
        let Some(n)=ferric_ath5k::packet::encap_and_protect_eth(&mut frame,eth,&self.ap,&self.sta,true,&self.connected.tk,&mut self.connected.sequence,&mut self.pn)else{return false;};
        let mut t=Transport{io,region,sta:self.sta,ap:self.ap,rx_index:self.rx_index};
        t.transmit(&frame[..n],0x0d)
    }
    pub fn receive(&mut self,io:&mut impl TransportIo,region:Region,out:&mut [u8])->Result<Option<usize>,Error>{
        let mut raw=[0u8;1600];let mut t=Transport{io,region,sta:self.sta,ap:self.ap,rx_index:self.rx_index};
        let result=t.receive_raw(&mut raw,1);self.rx_index=t.rx_index;
        let Some(n)=result?else{return Ok(None);};
        match ferric_ath5k::packet::receive_eth(out,&raw[..n],&self.sta,&self.ap,true){
            Some((ferric_ath5k::packet::RxEthAction::Deliver,n))=>Ok(Some(n)),
            // Fail closed until the rekey path is connected; never deliver EAPOL.
            Some((ferric_ath5k::packet::RxEthAction::Eapol,_))=>Err(Error::Handshake),
            _=>Ok(None)
        }
    }
}
