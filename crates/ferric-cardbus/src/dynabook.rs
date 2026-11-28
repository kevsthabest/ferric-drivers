//! Physical capture-derived preflight, not firmware authorization.
//! Exact BAR bases and bridge windows from PCIEVID.TXT (2026-11-28).
//! Keep all sizes that are not proven out of the authorization evidence.
use crate::audit::PciRead;
use crate::resources::Range;
pub const AGP:Range=Range{base:0xf0000000,len:0x04000000};
pub const GRAPHICS:Range=Range{base:0xf7f00000,len:0x06100000};
/// Maximum naturally aligned size of an assigned conventional memory BAR.
/// This is an upper bound, NOT measured size, and assumes compliant decoding.
/// Zero, I/O, reserved and 64-bit BAR encodings are deliberately rejected.
pub const fn upper_bound(raw:u32)->Option<Range>{
 if raw==u32::MAX||raw&7!=0{return None;}
 let base=raw&!15;
 if base==0{return None;}
 Some(Range{base,len:base&base.wrapping_neg()})
}
/// The capture's non-graphics endpoint memory BARs; graphics is already
/// covered by the full AGP bridge window. Do not use these as root windows.
pub const ENDPOINT_BARS:[u32;7]=[0xf7eff000,0xf7efe000,0xf7ec0000,0xf7ebf000,0xf7ebe000,0xf7ebdf00,0xf7ebdc00];
pub fn endpoint_bounds_clear(r:crate::SlotResources)->bool{
 let Ok(ranges)=crate::resources::validate(r,&[AGP,GRAPHICS],&[])else{return false;};
 ENDPOINT_BARS.iter().all(|&raw|upper_bound(raw).is_some_and(|b|ranges.iter().all(|r|!r.overlaps(b))))
}
const EXPECTED:&[(u8,u8,u8,u8,u32)]=&[
 (0,0,0,0,0x164410b9),(0,0,0,0x10,0xf0000008),(0,0,0,0xbc,8),
 (0,1,0,0,0x524710b9),(0,1,0,0x18,0x00010100),(0,1,0,0x20,0xfdf0f7f0),
 (0,6,0,0,0x545110b9),(0,6,0,0x14,0xf7eff000),
 (0,10,0,0,0x12298086),(0,10,0,0x10,0xf7efe000),(0,10,0,0x18,0xf7ec0000),
 (0,12,0,0,0x00351033),(0,12,0,0x10,0xf7ebf000),
 (0,12,1,0,0x00351033),(0,12,1,0x10,0xf7ebe000),
 (0,12,2,0,0x00e01033),(0,12,2,0x10,0xf7ebdf00),
 (0,17,0,0,0x06171179),(0,17,0,0x18,0x00030300),(0,17,1,0,0x06171179),(0,17,1,0x18,0x00040400),
 (0,18,0,0,0x08051179),(0,18,0,0x10,0xf7ebdc00),
 (1,0,0,0,0x88201023),(1,0,0,0x10,0xfc000000),(1,0,0,0x14,0xfbc00000),
 (1,0,0,0x18,0xf8000000),(1,0,0,0x1c,0xf7ff8000),
];
/// Exact boot fingerprint; false on a changed allocation. A match alone
/// cannot prove aperture sizes or host address routing.
pub fn matches_capture(io:&mut impl PciRead)->bool{
 EXPECTED.iter().all(|&(b,d,f,o,v)|io.read(b,d,f,o)==v)
}
#[cfg(test)]
mod tests{
 use super::*;
 struct Mock{change:bool}
 impl PciRead for Mock{fn read(&mut self,b:u8,d:u8,f:u8,o:u8)->u32{
  if self.change&&(b,d,f,o)==(0,10,0,0x18){return 0xf7d00000;}
  EXPECTED.iter().find(|&&(bb,dd,ff,oo,_)|(b,d,f,o)==(bb,dd,ff,oo)).map(|x|x.4).unwrap_or(u32::MAX)
 }}
 #[test]fn fingerprint_rejects_relocation(){
  assert!(matches_capture(&mut Mock{change:false}));
  assert!(!matches_capture(&mut Mock{change:true}));
 }
 #[test]fn conservative_bar_bounds(){
  assert_eq!(upper_bound(0xf7ec0000),Some(Range{base:0xf7ec0000,len:0x40000}));
  assert_eq!(upper_bound(0xf7ebdf00).unwrap().len,256);
  assert_eq!(upper_bound(0xf7ebdc00).unwrap().len,1024);
  for raw in [0,1,4,0xffffffff,0xf7e00002]{assert!(upper_bound(raw).is_none());}
  let mut r=crate::slot_resources(0).unwrap();assert!(endpoint_bounds_clear(r));
  r.bar0_phys=0xf7ec1000;assert!(!endpoint_bounds_clear(r));
 }
 #[test]fn known_windows_disjoint(){
  assert_eq!(u64::from(GRAPHICS.base)+u64::from(GRAPHICS.len)-1,0xfdffffff);
  let r=crate::resources::validate(crate::slot_resources(0).unwrap(),&[AGP,GRAPHICS],&[]).unwrap();
  assert_eq!(r[1].len,0x100000);
 }
}
