//! Explicitly supplied 256-bit random seed. No pretend entropy from RTC/PIT.
//! A qualified external random source must provide a fresh seed per boot.
//! This guard rejects accidental reuse within a loaded resident instance.
pub struct Guard{last:[u8;32],used:bool}
impl Guard{
    pub const fn new()->Self{Self{last:[0;32],used:false}}
    pub fn accept(&mut self,n:[u8;32])->bool{
        if n==[0;32]||n.iter().all(|b|*b==n[0])||(self.used&&self.last==n){return false;}
        self.last=n;self.used=true;true
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn refuses_reuse_and_constant_seed(){
        let mut g=Guard::new();let mut n=[0;32];n[0]=1;
        assert!(!g.accept([0;32]));assert!(!g.accept([7;32]));
        assert!(g.accept(n));assert!(!g.accept(n));n[1]=2;assert!(g.accept(n));
    }
}
