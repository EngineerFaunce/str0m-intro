use once_cell::sync::Lazy;
use std::net::IpAddr;
use systemstat::{Platform, System};

pub fn get_external_ip_address() -> IpAddr {
    let system = System::new();
    let networks = system.networks().unwrap();

    for net in networks.values() {
        for n in &net.addrs {
            if let systemstat::IpAddr::V4(v) = n.addr {
                if !v.is_loopback() && !v.is_link_local() && !v.is_broadcast() {
                    return IpAddr::V4(v);
                }
            }
        }
    }

    panic!("Found no usable network interface");
}

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
