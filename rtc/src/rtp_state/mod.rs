use rand::Rng;
use std::time::Instant;

use str0m::rtp::SeqNo;

#[derive(Debug)]
pub struct RtpState {
    seq_no: u16,
    ts_base: u32,
    start_time: Instant,
}

impl RtpState {
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        Self {
            seq_no: rng.r#gen::<u16>(),
            ts_base: rng.r#gen::<u32>(),
            start_time: Instant::now(),
        }
    }

    pub fn next(&mut self) -> (SeqNo, u32) {
        let seq = self.seq_no;
        self.seq_no = self.seq_no.wrapping_add(1);

        // 90kHz RTP clock for H.264 video
        let elapsed_90khz = (self.start_time.elapsed().as_micros() * 90) as u32;
        let ts = self.ts_base.wrapping_add(elapsed_90khz);

        (SeqNo::from(seq as u64), ts)
    }
}
