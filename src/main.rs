mod dongle;
mod install;
mod uhid;

use std::io;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use dongle::{Battery, Dongle};
use uhid::VirtualBattery;

const NAME: &str = "Razer BlackShark V2 HyperSpeed";
/// Becomes the power_supply name: hid-blackshark-v2-hyperspeed-battery
const UNIQ: &str = "blackshark-v2-hyperspeed";
const VENDOR: u32 = 0x1532;
const PRODUCT: u32 = 0x0565;

/// hid-input may drop the first input report if it arrives before the virtual device
/// finished probing, so repeat it shortly after creation.
const RESEND_AFTER_CREATE: Duration = Duration::from_secs(2);

const USAGE: &str = "\
usage: blackshark-battery <command>

commands:
  run [--interval SECONDS]  poll the headset and publish its battery (default every 60s)
  status                    print the headset battery state once
  install [--yes]           install the binary, systemd service and udev rule (asks for root)
  uninstall [--yes]         remove them again

All commands except install/uninstall need root (the service runs as root).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["--version" | "-V"] => {
            println!("blackshark-battery {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        ["--help" | "-h" | "help"] => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        ["install", rest @ ..] => {
            return parse_yes(rest).map_or_else(usage, |yes| ExitCode::from(install::install(yes)));
        }
        ["uninstall", rest @ ..] => {
            return parse_yes(rest).map_or_else(usage, |yes| ExitCode::from(install::uninstall(yes)));
        }
        ["status"] => print_status(),
        ["run"] => run(Duration::from_secs(60)),
        ["run", "--interval", secs] => match secs.parse::<u64>() {
            Ok(s) if s > 0 => run(Duration::from_secs(s)),
            _ => return usage(),
        },
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn parse_yes(rest: &[&str]) -> Option<bool> {
    match rest {
        [] => Some(false),
        ["--yes" | "-y"] => Some(true),
        _ => None,
    }
}

fn print_status() -> io::Result<()> {
    let Some(mut dongle) = Dongle::open()? else {
        return Err(io::Error::other("dongle 1532:0565 not found"));
    };
    match dongle.battery()? {
        Some(b) => println!("{}%{}", b.percent, if b.charging { " charging" } else { "" }),
        None => println!("headset not responding"),
    }
    Ok(())
}

fn run(interval: Duration) -> io::Result<()> {
    let mut dongle: Option<Dongle> = None;
    let mut battery: Option<VirtualBattery> = None;
    let mut last: Option<Battery> = None;
    let mut next_poll = Instant::now();
    let mut resend_at: Option<Instant> = None;

    loop {
        let now = Instant::now();
        if now >= next_poll {
            next_poll = now + interval;
            let state = poll_headset(&mut dongle);
            if state != last {
                match state {
                    Some(b) => eprintln!("battery {}%{}", b.percent, if b.charging { ", charging" } else { "" }),
                    None => eprintln!("headset unavailable"),
                }
                last = state;
            }
            match state {
                Some(b) => {
                    if battery.is_none() {
                        battery = Some(VirtualBattery::create(NAME, UNIQ, VENDOR, PRODUCT)?);
                        resend_at = Some(now + RESEND_AFTER_CREATE);
                    }
                    battery.as_mut().unwrap().update(b)?;
                }
                // Removing the virtual device makes the applet drop the entry.
                None => battery = None,
            }
        }
        if let (Some(at), Some(vb), Some(b)) = (resend_at, battery.as_mut(), last)
            && now >= at
        {
            vb.update(b)?;
            resend_at = None;
        }

        let wake = resend_at.map_or(next_poll, |r| r.min(next_poll));
        let timeout = wake.saturating_duration_since(Instant::now());
        match battery.as_mut() {
            Some(vb) => {
                if dongle::poll_readable(vb.fd(), timeout)? {
                    vb.handle_event()?;
                }
            }
            None => std::thread::sleep(timeout),
        }
    }
}

/// Query the headset, (re)opening the dongle as needed. `None` when the dongle is
/// unplugged or the headset is off.
fn poll_headset(dongle: &mut Option<Dongle>) -> Option<Battery> {
    if dongle.is_none() {
        *dongle = Dongle::open().unwrap_or_else(|e| {
            eprintln!("open dongle: {e}");
            None
        });
    }
    match dongle.as_mut()?.battery() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("dongle: {e}");
            *dongle = None;
            None
        }
    }
}
