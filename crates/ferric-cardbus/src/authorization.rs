//! Resource authorization is distinct from a successful read-only PCI audit.
//! Inputs must be established independently from firmware/host bridge evidence.
use crate::{SlotResources,resources::{Range,validate}};
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum Error{Layout,MissingWindow,Occupied,Incomplete,Slot}
pub struct Evidence<'a>{
    pub root_windows:&'a [Range],
    pub occupied:&'a [Range],
    pub occupied_buses:&'a [u8],
    pub complete:bool,
}
fn contains(a:Range,b:Range)->bool{
    a.valid()&&b.valid()&&a.base<=b.base&&u64::from(a.base)+u64::from(a.len)>=u64::from(b.base)+u64::from(b.len)
}
/// Pure check only. The caller must independently establish provenance
/// and completeness; passing fabricated ranges cannot authorize hardware.
pub fn check(slot:u8,r:SlotResources,e:&Evidence<'_>)->Result<(),Error>{
    if slot!=0{return Err(Error::Slot);}
    if !e.complete||e.root_windows.is_empty()||e.occupied.is_empty(){return Err(Error::Incomplete);}
    let ranges=validate(r,&[],e.occupied_buses).map_err(|_|Error::Layout)?;
    if e.root_windows.iter().any(|w|!w.valid())||e.occupied.iter().any(|w|!w.valid()){return Err(Error::Incomplete);}
    for r in ranges{
        if !e.root_windows.iter().any(|w|contains(*w,r)){return Err(Error::MissingWindow);}
        if e.occupied.iter().any(|w|w.overlaps(r)){return Err(Error::Occupied);}
    }
    Ok(())
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn no_authorization_from_empty_or_partial_evidence(){
        let r=crate::slot_resources(0).unwrap();
        let occupied=[Range{base:0xf7eff000,len:4096}];
        let windows=[Range{base:0xf7c00000,len:0x300000}];
        let mut e=Evidence{root_windows:&windows,occupied:&occupied,occupied_buses:&[1,3,4],complete:false};
        assert_eq!(check(0,r,&e),Err(Error::Incomplete));
        e.complete=true;assert_eq!(check(0,r,&e),Ok(()));
        assert_eq!(check(1,r,&e),Err(Error::Slot));
        let bad=[Range{base:0xf7c00000,len:0x200000}];e.root_windows=&bad;
        assert_eq!(check(0,r,&e),Err(Error::MissingWindow));
        e.root_windows=&windows;
        let conflict=[Range{base:0xf7cffff0,len:32}];e.occupied=&conflict;
        assert_eq!(check(0,r,&e),Err(Error::Occupied));
    }
}
