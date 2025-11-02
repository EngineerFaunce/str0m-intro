use anyhow::Result;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSinkCallbacks};
use tokio::sync::mpsc::Sender;
use std::time::Duration;
use tracing::{debug, error, trace};

pub fn stream_test_video(sender_channel: Sender<Vec<u8>>) -> Result<()> {
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

    // * Set up appsink to capture RTP packets
    let appsink = sink.clone().dynamic_cast::<AppSink>().unwrap();
    appsink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(async move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let data = map.as_slice();

                if let Err(e) = sender_channel.send(data.to_vec()).await {
                    return Err(gst::FlowError::Eos);
                }

                Ok(gstreamer::FlowSuccess::Ok)
            })
            .build(),
    );

    // TODO: control the pipeline state externally
    pipeline.set_state(gst::State::Playing)?;
    trace!("GStreamer pipeline started");

    let bus = pipeline.bus().unwrap();

    loop {
        if let Some(msg) = bus.pop() {
            match msg.view() {
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
                _ => {
                    debug!("Other message: {:?}", msg);
                }
            }
        }

        // Small sleep to prevent busy waiting
        std::thread::sleep(Duration::from_millis(1));
    }

    pipeline.set_state(gst::State::Null)?;
    trace!("GStreamer pipeline stopped");

    Ok(())
}
