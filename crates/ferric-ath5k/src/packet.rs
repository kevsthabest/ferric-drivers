//! Shared Ethernet/802.11 packet boundary. No DOS interrupt hook here.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RxEthAction {
    /// Deliver to the Crynwr packet-driver client.
    Deliver,
    /// Unicast EAPOL-Key PDU addressed to our station; feed to `Wpa2Handshake`.
    Eapol,
    /// Drop without delivering (plaintext on WPA2 session, self-broadcast echo,
    /// or non-unicast EAPOL).
    Drop,
}

pub fn classify_rx_eth_frame(
    eth: &[u8],
    sta_mac: &[u8; 6],
    session_protected: bool,
    frame_protected: bool,
) -> RxEthAction {
    if eth.len() < 14 {
        return RxEthAction::Drop;
    }
    if session_protected && !frame_protected {
        return RxEthAction::Drop;
    }
    if eth[12] == 0x88 && eth[13] == 0x8E {
        if eth.len() >= 18 && eth[..6] == *sta_mac {
            return RxEthAction::Eapol;
        }
        return RxEthAction::Drop;
    }
    if (eth[0] & 0x01) != 0 && eth[6..12] == *sta_mac {
        return RxEthAction::Drop;
    }
    RxEthAction::Deliver
}

pub fn encap_and_protect_eth(
    out_dot11: &mut [u8],
    eth: &[u8],
    bssid: &[u8; 6],
    sta_mac: &[u8; 6],
    protected: bool,
    ptk_tk: &[u8; 16],
    tx_seq: &mut u16,
    tx_pn: &mut u64,
) -> Option<usize> {
    let ccmp_pn = if protected {
        let pn = *tx_pn;
        if pn==0 || pn >= (1u64<<48) {return None;}
        *tx_pn = pn + 1;
        Some((pn, 0u8))
    } else {
        None
    };
    let mut dlen = wifi_core::frame::encap_eth_to_80211(
        out_dot11,
        eth,
        bssid,
        sta_mac,
        *tx_seq,
        ccmp_pn,
    );
    *tx_seq = tx_seq.wrapping_add(1) & 0x0FFF;
    if dlen == 0 {
        return None;
    }
    if protected {
        dlen = wifi_core::aes::ccmp_encrypt_frame(ptk_tk, out_dot11, dlen)?;
    }
    Some(dlen)
}

/// Validate a decrypted receive frame, strip FCS and CCMP MIC, and
/// classify supplicant traffic separately from packet-driver delivery.
/// Caller must verify descriptor success and hardware decryption.
pub fn receive_eth(out:&mut [u8],raw:&[u8],sta:&[u8;6],ap:&[u8;6],protected:bool)->Option<(RxEthAction,usize)>{
    if raw.len()<36||raw.len()>1600{return None;}
    let (h,_)=wifi_core::frame::parse_header(raw)?;
    if h.addr2!=*ap||!h.from_ds||h.to_ds||h.frame_type!=wifi_core::frame::TYPE_DATA{return None;}
    if h.addr1!=*sta&&h.addr1[0]&1==0{return None;}
    if protected&&!h.protected{return None;}
    let tail=if h.protected{12}else{4};
    let n=wifi_core::frame::decap_80211_to_eth(out,&raw[..raw.len()-tail],ap)?;
    let action=classify_rx_eth_frame(&out[..n],sta,protected,h.protected);
    if action==RxEthAction::Deliver&&n<60{
        if out.len()<60{return None;}
        out[n..60].fill(0);return Some((action,60));
    }
    Some((action,n))
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn pn_exhaustion_fails_closed(){
        let mut out=[0u8;1600];let mut seq=0;let mut pn=1u64<<48;
        assert_eq!(encap_and_protect_eth(&mut out,&[0;60],&[2;6],&[4;6],true,&[0;16],&mut seq,&mut pn),None);
        assert_eq!(pn,1u64<<48);
    }
    #[test]fn eapol_is_not_application_traffic(){
        let sta=[2;6];let mut eth=[0u8;60];eth[..6].copy_from_slice(&sta);eth[12..14].copy_from_slice(&[0x88,0x8e]);
        assert_eq!(classify_rx_eth_frame(&eth,&sta,true,true),RxEthAction::Eapol);
        assert_eq!(classify_rx_eth_frame(&eth,&sta,true,false),RxEthAction::Drop);
    }
}
