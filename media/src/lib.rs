use anyhow::{Result, anyhow};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSinkCallbacks};
use rtc::OutboundRtpPacket;
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, trace, warn};

fn parse_rtp_packet(data: &[u8]) -> Option<OutboundRtpPacket> {
    if data.len() < 12 {
        return None;
    }

    let version = data[0] >> 6;
    if version != 2 {
        return None;
    }

    let csrc_count = (data[0] & 0x0f) as usize;
    let has_extension = (data[0] & 0x10) != 0;
    let marker = (data[1] & 0x80) != 0;
    let payload_type = data[1] & 0x7f;
    let sequence_number = u16::from_be_bytes([data[2], data[3]]);
    let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

    let mut header_len = 12 + (csrc_count * 4);
    if data.len() < header_len {
        return None;
    }

    if has_extension {
        if data.len() < header_len + 4 {
            return None;
        }
        let ext_len_words =
            u16::from_be_bytes([data[header_len + 2], data[header_len + 3]]) as usize;
        header_len += 4 + (ext_len_words * 4);
        if data.len() < header_len {
            return None;
        }
    }

    trace!(
        "Parsed RTP packet - pt: {}, seq_no: {}, ts: {}",
        payload_type, sequence_number, timestamp
    );
    Some(OutboundRtpPacket {
        payload_type,
        sequence_number,
        timestamp,
        marker,
        payload: data[header_len..].to_vec(),
    })
}

fn parse_sample(sample: &gst::Sample) -> Option<OutboundRtpPacket> {
    let buffer = sample.buffer()?;
    let map = buffer.map_readable().ok()?;
    parse_rtp_packet(map.as_slice())
}

fn build_test_video_pipeline() -> Result<(gst::Pipeline, AppSink)> {
    let pipeline = gst::Pipeline::default();
    let src = gst::ElementFactory::make("videotestsrc")
        .property("is-live", true)
        .property_from_str("pattern", "ball")
        .build()?;
    let conv = gst::ElementFactory::make("videoconvert").build()?;
    let enc = gst::ElementFactory::make("x264enc")
        .property_from_str("tune", "zerolatency")
        .build()?;
    let pay = gst::ElementFactory::make("rtph264pay").build()?;
    let sink = gst::ElementFactory::make("appsink")
        .property("emit-signals", true)
        .property("sync", false)
        .build()?;

    pipeline.add_many([&src, &conv, &enc, &pay, &sink])?;
    gst::Element::link_many([&src, &conv, &enc, &pay, &sink])?;

    let appsink = sink
        .dynamic_cast::<AppSink>()
        .map_err(|_| anyhow!("failed to cast sink element to AppSink"))?;

    Ok((pipeline, appsink))
}

pub async fn stream_test_video(sender_channel: Sender<OutboundRtpPacket>) -> Result<()> {
    gst::init()?;

    // * Set up GStreamer pipeline
    let (pipeline, appsink) = build_test_video_pipeline()?;

    appsink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;

                let Some(packet) = parse_sample(&sample) else {
                    warn!("Dropping malformed RTP packet from appsink");
                    return Ok(gstreamer::FlowSuccess::Ok);
                };

                match sender_channel.try_send(packet) {
                    Ok(_) => {
                        // trace!("Packet sent.");
                        Ok(gstreamer::FlowSuccess::Ok)
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        warn!("Media channel full, dropping packet");
                        Ok(gstreamer::FlowSuccess::Ok) // Drop packet but continue
                    }
                    Err(_) => {
                        error!("Channel closed");
                        Err(gst::FlowError::Eos)
                    }
                }
            })
            .build(),
    );

    // TODO: control the pipeline state externally
    pipeline.set_state(gst::State::Playing)?;
    trace!("GStreamer pipeline started");

    let bus = pipeline.bus().unwrap();

    loop {
        match bus.timed_pop(gst::ClockTime::from_mseconds(100)) {
            Some(msg) => match msg.view() {
                gst::MessageView::Eos(..) => {
                    debug!("End of stream");
                    break;
                }
                gst::MessageView::Error(err) => {
                    error!(
                        "Pipeline error from {:?}: {}",
                        err.src().map(|s| s.path_string()),
                        err.error()
                    );
                    break;
                }
                _ => {}
            },
            None => {} // Timeout, continue loop
        }
    }

    pipeline.set_state(gst::State::Null)?;
    trace!("GStreamer pipeline stopped");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet_bytes(with_extension: bool) -> Vec<u8> {
        let mut bytes = vec![
            if with_extension { 0x90 } else { 0x80 }, // V=2 (+ X=1 when extension)
            0xE0,                                     // M=1, PT=96
            0x12,
            0x34, // seq
            0xAA,
            0xBB,
            0xCC,
            0xDD, // timestamp
            0x11,
            0x22,
            0x33,
            0x44, // ssrc
        ];

        if with_extension {
            bytes.extend_from_slice(&[
                0xBE, 0xDE, // extension profile id
                0x00, 0x01, // extension length = 1 word (4 bytes)
                0xCA, 0xFE, 0xBA, 0xBE,
            ]);
        }

        bytes.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]);
        bytes
    }

    #[test]
    fn parse_valid_rtp_packet() {
        let parsed = parse_rtp_packet(&packet_bytes(false)).expect("valid RTP packet");
        assert_eq!(parsed.payload_type, 96);
        assert_eq!(parsed.sequence_number, 0x1234);
        assert_eq!(parsed.timestamp, 0xAABBCCDD);
        assert!(parsed.marker);
        assert_eq!(parsed.payload, vec![0x01, 0x02, 0x03, 0x04]);
    }

    #[test]
    fn parse_rejects_wrong_version() {
        let mut bytes = packet_bytes(false);
        bytes[0] = 0x40; // V=1
        assert!(parse_rtp_packet(&bytes).is_none());
    }

    #[test]
    fn parse_rejects_short_packet() {
        let bytes = vec![0u8; 8];
        assert!(parse_rtp_packet(&bytes).is_none());
    }

    #[test]
    fn parse_supports_header_extensions() {
        let parsed = parse_rtp_packet(&packet_bytes(true)).expect("valid RTP extension packet");
        assert_eq!(parsed.payload, vec![0x01, 0x02, 0x03, 0x04]);
    }

    #[test]
    fn parse_sample_extracts_rtp_data() {
        gst::init().expect("gstreamer init");
        let buffer = gst::Buffer::from_mut_slice(packet_bytes(false));
        let sample = gst::Sample::builder().buffer(&buffer).build();

        let parsed = parse_sample(&sample).expect("sample should parse");
        assert_eq!(parsed.sequence_number, 0x1234);
        assert_eq!(parsed.timestamp, 0xAABBCCDD);
    }
}
