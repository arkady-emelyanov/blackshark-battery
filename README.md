# blackshark-battery

[![CI](https://github.com/arkady-emelyanov/blackshark-battery/actions/workflows/ci.yml/badge.svg)](https://github.com/arkady-emelyanov/blackshark-battery/actions/workflows/ci.yml)

Show the battery level of a Razer BlackShark V2 HyperSpeed headset in your desktop's power applet on Linux, next to your wireless keyboard and mouse. No Razer Synapse needed.

<p align="center">
  <img src="docs/power-applet.png" alt="Cinnamon power applet listing a Magic Keyboard, an APC UPS and the Razer BlackShark V2 HyperSpeed at 89%" width="450"><br>
  <sub><b>Cinnamon power applet</b>: the headset listed with its own icon, next to a keyboard and a UPS.</sub>
</p>

The headset only reports its battery through Razer's vendor protocol, which desktop tools don't understand. blackshark-battery reads it from the 2.4 GHz dongle and republishes it as a standard HID battery, so the kernel exposes it as a `power_supply` and UPower lists it as a headset. Anything that uses UPower picks it up: the Cinnamon, GNOME and KDE power applets, `upower -d`, and so on.

## Supported hardware

| Device | USB ID | Status |
|---|---|---|
| BlackShark V2 HyperSpeed, 2.4 GHz dongle | `1532:0565` | Tested |
| BlackShark V2 HyperSpeed, USB-C cable | `1532:056e` | Tested: while charging from the computer, the headset answers over the cable instead |
| BlackShark V2 Pro and other models | | Not supported: they use a different protocol |

`lsusb -d 1532:` shows which one you have.

## Install

Download the static binary for your machine from the [latest release](https://github.com/arkady-emelyanov/blackshark-battery/releases/latest) and run its installer:

```
curl -Lo /tmp/blackshark-battery https://github.com/arkady-emelyanov/blackshark-battery/releases/latest/download/blackshark-battery-$(uname -m)-linux
chmod +x /tmp/blackshark-battery
/tmp/blackshark-battery install
```

Builds are available for `x86_64` and `aarch64`; each has a `.sha256` file next to it. To build from source instead:

```
cargo build --release --target x86_64-unknown-linux-musl
target/x86_64-unknown-linux-musl/release/blackshark-battery install
```

The headset appears in the power applet a few seconds later, as long as it's switched on.

### What the installer does, and why it needs root

`install` shows what it is about to write and asks for your password once (through `pkexec`, or `sudo` if `pkexec` isn't available). It then, as root:

- copies the binary to `/usr/local/bin/blackshark-battery`;
- writes `/etc/systemd/system/blackshark-battery.service`, then enables and starts it;
- writes `/etc/udev/rules.d/60-blackshark-battery.rules`, which tags the virtual battery's input node as a headset so UPower shows the right icon and type.

The service runs as root because it needs two devices that only root can use: the dongle's `/dev/hidrawN` node, to ask the headset for its battery level, and `/dev/uhid`, to create the virtual battery. Granting `/dev/uhid` to a user account instead would let any program running as that user create fake keyboards, so it stays root-only. The service is sandboxed: it can't touch any other device, the network, or your home directory.

`blackshark-battery uninstall` stops the service and removes all three files.

## Use

- `sudo blackshark-battery status` prints the current level, e.g. `89%` or `100% charging`.
- `journalctl -u blackshark-battery` shows level changes and when the headset comes and goes.
- `upower -d` lists it as `Razer BlackShark V2 HyperSpeed`, type `headset`.

The service checks the headset every 5 seconds, so changes show up in the applet within a few seconds. When the headset is switched off or the dongle is unplugged, the entry disappears from the applet and comes back when the headset is on again.

## How it works

1. Every 5 seconds it sends two read commands over the vendor HID interface: battery level (`0x21`) and charging state (`0x2a`). Normally they go to the dongle, which relays them to the headset over the air. While the headset is plugged into the computer by cable it stops answering through the dongle, so the commands go over the cable instead; the device is looked up afresh on every check, so switching back and forth needs nothing from you.
2. It creates a virtual HID device through `/dev/uhid` whose report descriptor has a standard Battery Strength and Charging usage, and sends the values as input reports.
3. The kernel's `hid-input` turns that into `/sys/class/power_supply/hid-blackshark-v2-hyperspeed-battery`, which UPower picks up like any Bluetooth mouse battery.

The dongle protocol was reverse-engineered by [justik13/razer-blackshark-v2-hyperspeed-webhid](https://github.com/justik13/razer-blackshark-v2-hyperspeed-webhid). blackshark-battery only ever sends read commands: it doesn't change any headset setting.

## Develop

```
cargo test
cargo build --release
sudo target/release/blackshark-battery status
sudo target/release/blackshark-battery run --interval 10
```
