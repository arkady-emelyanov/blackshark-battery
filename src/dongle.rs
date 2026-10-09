//! Vendor HID protocol of the BlackShark V2 HyperSpeed.
//!
//! The headset is reachable two ways, both MediaTek chips taking 63-byte frames on
//! report ID 0x02 of a vendor interface:
//! - the 2.4 GHz dongle (1532:0565), which relays frames addressed to domain 0x80
//!   over the air to the headset;
//! - the headset itself over its USB-C cable (1532:056e), domain 0x00. While the
//!   cable is plugged in the headset stops answering through the dongle.
//!
//! Protocol reverse-engineered by
//! https://github.com/justik13/razer-blackshark-v2-hyperspeed-webhid

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// (uevent HID_ID, command domain), cable first: it is the only one answering while
/// plugged in.
const DEVICES: [(&str, u8); 2] = [
    ("HID_ID=0003:00001532:0000056E", 0x00),
    ("HID_ID=0003:00001532:00000565", 0x80),
];
/// Usage Page 0xFF14: the collection that carries report ID 0x02 command frames.
const VENDOR_PAGE: [u8; 3] = [0x06, 0x14, 0xff];

const REPORT_ID: u8 = 0x02;
const FRAME_LEN: usize = 63;
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);

const CMD_BATTERY: u8 = 0x21;
const CMD_CHARGING: u8 = 0x2a;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

pub struct Dongle {
    file: File,
    domain: u8,
    seq: u8,
}

impl Dongle {
    /// Find the headset's vendor hidraw node, over the cable or the dongle, and open it.
    pub fn open() -> io::Result<Option<Self>> {
        let Some((path, domain)) = find_hidraw()? else {
            return Ok(None);
        };
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        Ok(Some(Self { file, domain, seq: 0 }))
    }

    /// Ask the headset for its battery state. `None` means the headset did not answer
    /// (powered off or out of range).
    pub fn battery(&mut self) -> io::Result<Option<Battery>> {
        let Some(percent) = self.query(CMD_BATTERY)? else {
            return Ok(None);
        };
        if percent > 100 {
            return Ok(None);
        }
        let charging = self.query(CMD_CHARGING)?.is_some_and(|v| v > 0);
        Ok(Some(Battery { percent, charging }))
    }

    /// Send a GET command and return the first payload byte of the matching reply.
    fn query(&mut self, cmd: u8) -> io::Result<Option<u8>> {
        let seq = 0x60 | (self.seq & 0x1f);
        self.seq = self.seq.wrapping_add(1);
        self.file.write_all(&frame(seq, cmd, self.domain))?;

        let deadline = Instant::now() + REPLY_TIMEOUT;
        let mut buf = [0u8; 64];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !poll_readable(&self.file, left)? {
                return Ok(None);
            }
            let n = self.file.read(&mut buf)?;
            // hidraw prefixes the report ID when the device uses numbered reports.
            let d = match buf[..n].first() {
                Some(&REPORT_ID) => &buf[1..n],
                _ => &buf[..n],
            };
            // [1] seq, [9] cmd, [10] ack flag, [11] payload count, [12..] payload
            if d.len() >= 13 && d[1] == seq && d[9] == cmd && d[10] == 0x01 && d[11] >= 1 {
                return Ok(Some(d[12]));
            }
        }
    }
}

fn frame(seq: u8, cmd: u8, domain: u8) -> [u8; FRAME_LEN + 1] {
    let mut p = [0u8; FRAME_LEN];
    p[1] = seq;
    p[5] = 0x04; // payload length of a query
    p[8] = domain;
    p[9] = cmd;
    // The dongle drops frames whose XOR checksum (seeded with the report ID) mismatches.
    p[61] = p[..61].iter().fold(REPORT_ID, |x, b| x ^ b);

    let mut out = [0u8; FRAME_LEN + 1];
    out[0] = REPORT_ID;
    out[1..].copy_from_slice(&p);
    out
}

fn find_hidraw() -> io::Result<Option<(PathBuf, u8)>> {
    let mut found: Vec<(usize, PathBuf, u8)> = Vec::new();
    for entry in fs::read_dir("/sys/class/hidraw")? {
        let dev = entry?.path().join("device");
        let Ok(uevent) = fs::read_to_string(dev.join("uevent")) else {
            continue;
        };
        let Some(rank) = DEVICES.iter().position(|(id, _)| uevent.lines().any(|l| l == *id)) else {
            continue;
        };
        let rdesc = fs::read(dev.join("report_descriptor")).unwrap_or_default();
        if rdesc.windows(VENDOR_PAGE.len()).any(|w| w == VENDOR_PAGE) {
            found.push((rank, PathBuf::from("/dev").join(entry_name(&dev)?), DEVICES[rank].1));
        }
    }
    Ok(found.into_iter().min_by_key(|(rank, ..)| *rank).map(|(_, path, domain)| (path, domain)))
}

/// `/sys/class/hidraw/hidrawN/device` -> `hidrawN`
fn entry_name(dev: &std::path::Path) -> io::Result<std::ffi::OsString> {
    dev.parent()
        .and_then(|p| p.file_name())
        .map(ToOwned::to_owned)
        .ok_or_else(|| io::Error::other("bad hidraw sysfs path"))
}

pub(crate) fn poll_readable(file: &File, timeout: Duration) -> io::Result<bool> {
    let mut pfd = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "hidraw device gone"));
    }
    Ok(rc > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_checksum() {
        let f = frame(0x60, CMD_BATTERY, 0x80);
        assert_eq!(f[0], REPORT_ID);
        let p = &f[1..];
        assert_eq!(p.len(), FRAME_LEN);
        assert_eq!((p[1], p[5], p[8], p[9]), (0x60, 0x04, 0x80, 0x21));
        // 0x02 ^ 0x60 ^ 0x04 ^ 0x80 ^ 0x21
        assert_eq!(p[61], 0xc7);
    }
}
