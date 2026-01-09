use anyhow::Result;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSinkCallbacks};
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, trace};

pub async fn stream_test_video(sender_channel: Sender<Vec<u8>>) -> Result<()> {
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

                // Validate packet size (typical RTP packets are < 1500 bytes)
                if data.len() > 1500 {
                    tracing::warn!("Unusually large RTP packet: {} bytes", data.len());
                }

                match sender_channel.try_send(data.to_vec()) {
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
