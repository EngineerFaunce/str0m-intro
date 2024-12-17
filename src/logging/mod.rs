pub fn init_log() {
    use std::env;
    // use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    use tracing_subscriber::{fmt, prelude::*};

    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "http_post=debug,str0m=debug");
    }

    tracing_subscriber::registry()
        .with(fmt::layer())
        // .with(EnvFilter::from_default_env())
        .init();
}