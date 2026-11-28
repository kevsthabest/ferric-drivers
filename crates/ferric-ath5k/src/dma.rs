//! AR2413 16 KiB DMA geometry and receive descriptor initialization.
pub const WIFI_DMA_SIZE: usize = 16384; // 16 KiB (1024 DOS paragraphs)
pub const RX_DESC_COUNT: usize = 8;
pub const RX_DESC_STRIDE: usize = 64;
pub const RX_BUF_BASE: usize = 0x0400; // 1 KiB-aligned after RX (0x000..0x200) + TX (0x300) descs
pub const RX_BUF_SIZE: usize = 1600; // 8 * 1600 = 0x3200 -> 0x0400..0x3600 (before TX_BUF 0x3800)
pub const TX_DESC_OFF: usize = 0x0300;
pub const TX_BUF_OFF: usize = 0x3800; // 2 KiB after the 8 RX buffers (0x0400..0x3600)

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub struct Region {pub linear:u32,pub physical:u32}
impl Region{
    pub fn new(linear:u32,physical:u32)->Option<Self>{
        if linear==0||physical==0||linear&15!=0||physical&15!=0||
           linear.checked_add(WIFI_DMA_SIZE as u32-1).is_none()||
           physical.checked_add(WIFI_DMA_SIZE as u32-1).is_none(){return None;}
        Some(Self{linear,physical})
    }
    pub fn rx_descriptor(&self,i:usize)->Option<[u32;6]>{
        if i>=RX_DESC_COUNT{return None;}
        Some([self.physical+(((i+1)%RX_DESC_COUNT)*RX_DESC_STRIDE) as u32,
            self.physical+(RX_BUF_BASE+i*RX_BUF_SIZE) as u32,0,RX_BUF_SIZE as u32,0,0])
    }
}
pub fn initialize_rx(region:Region,mut write:impl FnMut(usize,u32)){
    for i in 0..RX_DESC_COUNT{
        if let Some(words)=region.rx_descriptor(i){
            for (j,v) in words.into_iter().enumerate(){write(i*RX_DESC_STRIDE+j*4,v);}
        }
    }
}
const _:()={
    assert!(RX_DESC_COUNT*RX_DESC_STRIDE<=TX_DESC_OFF);
    assert!(TX_DESC_OFF+RX_DESC_STRIDE<=RX_BUF_BASE);
    assert!(RX_BUF_BASE+RX_DESC_COUNT*RX_BUF_SIZE<=TX_BUF_OFF);
    assert!(TX_BUF_OFF<WIFI_DMA_SIZE);
};
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn geometry_and_wrap(){
        let r=Region::new(0x20000,0x40000).unwrap();
        let mut words=[0u32;WIFI_DMA_SIZE/4];
        initialize_rx(r,|o,v|words[o/4]=v);
        assert_eq!(words[0],0x40040);
        assert_eq!(words[1],0x40400);
        assert_eq!(words[7*RX_DESC_STRIDE/4],0x40000);
        assert_eq!(words[7*RX_DESC_STRIDE/4+1],0x42fc0);
        assert_eq!(words[3],1600);
        assert!(r.rx_descriptor(8).is_none());
    }
    #[test]fn rejects_invalid_region(){
        for (l,p) in [(0,0x40000),(0x20001,0x40000),(0x20000,0),(0x20000,0xfffffff0)]{assert!(Region::new(l,p).is_none());}
    }
}
