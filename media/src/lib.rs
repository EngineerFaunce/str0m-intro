use anyhow::Result;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSinkCallbacks};
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, trace, warn};

#[derive(Debug, Clone)]
pub struct RtpPacketData {
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub marker: bool,
    pub payload: Vec<u8>,
}

fn parse_rtp_packet(data: &[u8]) -> Option<RtpPacketData> {
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

    Some(RtpPacketData {
        payload_type,
        sequence_number,
        timestamp,
        marker,
        payload: data[header_len..].to_vec(),
    })
}

pub async fn stream_test_video(sender_channel: Sender<RtpPacketData>) -> Result<()> {
    gst::init()?;

    // * Set up GStreamer pipeline
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

    let appsink = sink.clone().dynamic_cast::<AppSink>().unwrap();
    appsink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let data = map.as_slice();

                let Some(packet) = parse_rtp_packet(data) else {
                    warn!("Dropping malformed RTP packet from appsink");
                    return Ok(gstreamer::FlowSuccess::Ok);
                };

                match sender_channel.try_send(packet) {
                    Ok(_) => Ok(gstreamer::FlowSuccess::Ok),
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!("Channel full, dropping packet");
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
