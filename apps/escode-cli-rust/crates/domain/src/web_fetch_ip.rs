//! WebFetch 出网判定的 IP 分类：ipaddr.js 1.9.1 `range() === "unicast"`，外加 TS 显式排除的段。
//! 见 docs/specs/rust-webfetch.md。

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub(crate) fn parse_ip(host: &str) -> Option<IpAddr> {
    host.parse().ok()
}

/// ipaddr.js 1.9.1 的 `range() === "unicast"`，外加 TS 显式排除的 benchmark 与 special-use 段。
pub(crate) fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => match carried_v4(v6) {
            Some(v4) => public_v4(v4),
            None => public_v6(v6),
        },
    }
}

fn carried_v4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let octets = address.octets();
    let low = Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]);
    let mapped = octets[..10].iter().all(|b| *b == 0) && octets[10] == 0xff && octets[11] == 0xff;
    let dns64 = octets[..12] == [0, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0];
    (mapped || dns64).then_some(low)
}

fn public_v4(address: Ipv4Addr) -> bool {
    const BLOCKED: [([u8; 4], u32); 16] = [
        ([0, 0, 0, 0], 8),
        ([255, 255, 255, 255], 32),
        ([224, 0, 0, 0], 4),
        ([169, 254, 0, 0], 16),
        ([127, 0, 0, 0], 8),
        ([100, 64, 0, 0], 10),
        ([10, 0, 0, 0], 8),
        ([172, 16, 0, 0], 12),
        ([192, 168, 0, 0], 16),
        ([192, 0, 0, 0], 24),
        ([192, 0, 2, 0], 24),
        ([192, 88, 99, 0], 24),
        ([198, 51, 100, 0], 24),
        ([203, 0, 113, 0], 24),
        ([240, 0, 0, 0], 4),
        ([198, 18, 0, 0], 15),
    ];
    let value = u32::from(address);
    !BLOCKED.iter().any(|(network, bits)| {
        let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
        value & mask == u32::from(Ipv4Addr::from(*network)) & mask
    })
}

fn public_v6(address: Ipv6Addr) -> bool {
    const BLOCKED: [([u16; 8], u32); 16] = [
        ([0, 0, 0, 0, 0, 0, 0, 0], 128),
        ([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),
        ([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),
        ([0, 0, 0, 0, 0, 0, 0, 1], 128),
        ([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),
        ([0, 0, 0, 0, 0, 0xffff, 0, 0], 96),
        ([0, 0, 0, 0, 0xffff, 0, 0, 0], 96),
        ([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96),
        ([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),
        ([0x2001, 0, 0, 0, 0, 0, 0, 0], 32),
        ([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
        ([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),
        ([0x100, 0, 0, 0, 0, 0, 0, 0], 64),
        ([0x2001, 2, 0, 0, 0, 0, 0, 0], 48),
        ([0x2001, 0x10, 0, 0, 0, 0, 0, 0], 28),
        ([0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28),
    ];
    let value = u128::from(address);
    !BLOCKED.iter().any(|(network, bits)| {
        let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
        value & mask == u128::from(Ipv6Addr::from(*network)) & mask
    })
}
