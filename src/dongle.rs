//! Vendor HID protocol of the BlackShark V2 HyperSpeed 2.4 GHz dongle (1532:0565).
//!
//! The dongle is a MediaTek chip that takes 63-byte frames on report ID 0x02 of its
//! vendor interface. Frames addressed to domain 0x80 are relayed over the air to the
//! headset. Protocol reverse-engineered by
//! https://github.com/justik13/razer-blackshark-v2-hyperspeed-webhid

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const HID_ID: &str = "HID_ID=0003:00001532:00000565";
/// Usage Page 0xFF14: the collection that carries report ID 0x02 command frames.
const VENDOR_PAGE: [u8; 3] = [0x06, 0x14, 0xff];

const REPORT_ID: u8 = 0x02;
const FRAME_LEN: usize = 63;
const DOMAIN_HEADSET: u8 = 0x80;
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
    seq: u8,
}

impl Dongle {
    /// Find the dongle's vendor hidraw node and open it.
    pub fn open() -> io::Result<Option<Self>> {
        let Some(path) = find_hidraw()? else {
            return Ok(None);
        };
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        Ok(Some(Self { file, seq: 0 }))
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
        self.file.write_all(&frame(seq, cmd))?;

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

fn frame(seq: u8, cmd: u8) -> [u8; FRAME_LEN + 1] {
    let mut p = [0u8; FRAME_LEN];
    p[1] = seq;
    p[5] = 0x04; // payload length of a query
    p[8] = DOMAIN_HEADSET;
    p[9] = cmd;
    // The dongle drops frames whose XOR checksum (seeded with the report ID) mismatches.
    p[61] = p[..61].iter().fold(REPORT_ID, |x, b| x ^ b);

    let mut out = [0u8; FRAME_LEN + 1];
    out[0] = REPORT_ID;
    out[1..].copy_from_slice(&p);
    out
}

fn find_hidraw() -> io::Result<Option<PathBuf>> {
    for entry in fs::read_dir("/sys/class/hidraw")? {
        let dev = entry?.path().join("device");
        let Ok(uevent) = fs::read_to_string(dev.join("uevent")) else {
            continue;
        };
        if !uevent.lines().any(|l| l == HID_ID) {
            continue;
        }
        let rdesc = fs::read(dev.join("report_descriptor")).unwrap_or_default();
        if rdesc.windows(VENDOR_PAGE.len()).any(|w| w == VENDOR_PAGE) {
            let name = entry_name(&dev)?;
            return Ok(Some(PathBuf::from("/dev").join(name)));
        }
    }
    Ok(None)
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
        let f = frame(0x60, CMD_BATTERY);
        assert_eq!(f[0], REPORT_ID);
        let p = &f[1..];
        assert_eq!(p.len(), FRAME_LEN);
        assert_eq!((p[1], p[5], p[8], p[9]), (0x60, 0x04, 0x80, 0x21));
        // 0x02 ^ 0x60 ^ 0x04 ^ 0x80 ^ 0x21
        assert_eq!(p[61], 0xc7);
    }
}
