//! Bounded command submission. A timeout before submission is different
//! from a timeout after submission: the latter may have started DMA.
pub trait ScbIo {
    fn command(&mut self) -> u8;
    fn set_pointer(&mut self, physical: u32);
    fn set_command(&mut self, command: u8);
    fn tick(&mut self) -> u32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandError { Busy, AcceptanceUnknown }

pub fn wait_idle(io: &mut impl ScbIo, ticks: u32, polls: u32) -> bool {
    let start = io.tick();
    for _ in 0..polls {
        if io.command() == 0 { return true; }
        if io.tick().wrapping_sub(start) > ticks { return false; }
    }
    false
}

pub fn submit(io: &mut impl ScbIo, pointer: u32, command: u8,
              ticks: u32, polls: u32) -> Result<(), CommandError> {
    if !wait_idle(io, ticks, polls) { return Err(CommandError::Busy); }
    io.set_pointer(pointer);
    io.set_command(command);
    if !wait_idle(io, ticks, polls) { return Err(CommandError::AcceptanceUnknown); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Mock { busy: bool, hang_after: bool, writes: u8, gp: u32, reads: u32 }
    impl ScbIo for Mock {
        fn command(&mut self) -> u8 { self.reads += 1; u8::from(self.busy) }
        fn set_pointer(&mut self, p: u32) { self.gp = p; self.writes += 1; }
        fn set_command(&mut self, _: u8) { self.writes += 1; self.busy = self.hang_after; }
        fn tick(&mut self) -> u32 { 0 } // frozen BIOS tick, as in an ISR
    }
    #[test]
    fn busy_never_overwrites_command_or_pointer() {
        let mut m = Mock { busy: true, hang_after: false, writes: 0, gp: 0, reads: 0 };
        assert_eq!(submit(&mut m, 42, 0x10, 2, 8), Err(CommandError::Busy));
        assert_eq!(m.writes, 0);
        assert_eq!(m.reads, 8);
    }
    #[test]
    fn distinguishes_post_submission_timeout() {
        let mut m = Mock { busy: false, hang_after: true, writes: 0, gp: 0, reads: 0 };
        assert_eq!(submit(&mut m, 42, 0x10, 2, 8), Err(CommandError::AcceptanceUnknown));
        assert_eq!(m.writes, 2);
        assert_eq!(m.gp, 42);
    }
    #[test]
    fn normal_submission() {
        let mut m = Mock { busy: false, hang_after: false, writes: 0, gp: 0, reads: 0 };
        assert_eq!(submit(&mut m, 42, 0x10, 2, 8), Ok(()));
        assert_eq!(m.writes, 2);
        assert_eq!(m.reads, 2);
    }
}
