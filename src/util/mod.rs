use once_cell::sync::Lazy;
use std::net::IpAddr;
use systemstat::{Platform, System};

static HOST_ADDRESS: Lazy<IpAddr> = Lazy::new(|| {
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
});

pub fn get_external_ip_address() -> IpAddr {
    *HOST_ADDRESS
}