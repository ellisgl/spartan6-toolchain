//! s6-load: load a bitstream onto a Mojo V3 through its AVR (USB serial).
//!
//!     s6-load build/cylon-ise/cylon.bin            # RAM: lost on power cycle
//!     s6-load --flash build/cylon-ise/cylon.bin    # flash, verify, then load
//!
//! Follows the protocol of the official loader (embmicro/mojo-loader,
//! MojoLoader.java): reset the AVR by toggling DTR, send the command byte
//! ('R' RAM, 'F' flash, 'V' flash + verify), expect 'R', send the file length
//! as 4 bytes little-endian, expect 'O', send the raw .bin, expect 'D'. For
//! verify: send 'S', expect 0xAA, a 4-byte LE length (file length + 5), then
//! the file bytes echoed back. Finally 'L' loads the FPGA from flash and
//! expects 'D'.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::process::exit;
use std::thread::sleep;
use std::time::{Duration, Instant};

use clap::Parser;

#[derive(Parser)]
#[command(about = "Load a bitstream onto a Mojo V3 through its AVR (USB serial)")]
struct Args {
    /// Raw bitstream (bitgen -g Binary:Yes, or s6-bitgen's output).
    bin: String,
    #[arg(long, default_value = "/dev/ttyACM0")]
    port: String,
    /// Write to flash (verified) instead of RAM.
    #[arg(long)]
    flash: bool,
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(1)
}

fn check(r: libc::c_int, what: &str) {
    if r < 0 {
        fail(format!("{what}: {}", std::io::Error::last_os_error()));
    }
}

struct Port(File);

impl Port {
    /// Raw 8N1 at 115200 baud.
    fn open(path: &str) -> Port {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(path)
            .unwrap_or_else(|e| fail(format!("{path}: {e}")));
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            check(libc::tcgetattr(f.as_raw_fd(), &mut t), "tcgetattr");
            libc::cfmakeraw(&mut t);
            t.c_cflag |= libc::CLOCAL | libc::CREAD;
            t.c_cflag &= !(libc::CSTOPB | libc::CRTSCTS);
            check(libc::cfsetspeed(&mut t, libc::B115200), "cfsetspeed");
            check(libc::tcsetattr(f.as_raw_fd(), libc::TCSANOW, &t), "tcsetattr");
        }
        Port(f)
    }

    fn dtr(&self, on: bool) {
        let bits: libc::c_int = libc::TIOCM_DTR;
        let req = if on { libc::TIOCMBIS } else { libc::TIOCMBIC };
        check(unsafe { libc::ioctl(self.0.as_raw_fd(), req, &bits) }, "set DTR");
    }

    fn flush_input(&self) {
        check(unsafe { libc::tcflush(self.0.as_raw_fd(), libc::TCIFLUSH) }, "tcflush");
    }

    /// Up to `n` bytes, giving up `timeout` after the call.
    fn read(&mut self, n: usize, timeout: Duration) -> Vec<u8> {
        let end = Instant::now() + timeout;
        let mut out = vec![];
        while out.len() < n {
            let left = end.saturating_duration_since(Instant::now());
            let mut pfd = libc::pollfd { fd: self.0.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let r = unsafe { libc::poll(&mut pfd, 1, left.as_millis() as libc::c_int) };
            if r == 0 {
                break;
            }
            check(r, "poll");
            let mut buf = vec![0; n - out.len()];
            let k = self.0.read(&mut buf).unwrap_or_else(|e| fail(e));
            out.extend_from_slice(&buf[..k]);
        }
        out
    }

    /// Up to `n` bytes, giving up once nothing has arrived for `idle`: for
    /// long transfers, whose total time depends on the link.
    fn read_idle(&mut self, n: usize, idle: Duration) -> Vec<u8> {
        let mut out = vec![];
        while out.len() < n {
            let chunk = self.read(n - out.len(), idle);
            if chunk.is_empty() {
                break;
            }
            out.extend_from_slice(&chunk);
        }
        out
    }

    fn write(&mut self, data: &[u8]) {
        self.0.write_all(data).unwrap_or_else(|e| fail(e));
    }

    fn expect(&mut self, want: u8, timeout_s: u64, what: &str) {
        let got = self.read(1, Duration::from_secs(timeout_s));
        if got != [want] {
            fail(format!("{what} (expected {:?}, got {got:?})", want as char));
        }
    }

    /// Same DTR pattern as MojoLoader.restartMojo(); ends with DTR high.
    fn restart(&self) {
        let ms5 = Duration::from_millis(5);
        self.dtr(false);
        sleep(ms5);
        for _ in 0..5 {
            self.dtr(false);
            sleep(ms5);
            self.dtr(true);
            sleep(ms5);
        }
    }
}

fn main() {
    let args = Args::parse();
    let data = std::fs::read(&args.bin).unwrap_or_else(|e| fail(format!("{}: {e}", args.bin)));
    if !data.windows(4).any(|w| w == [0xaa, 0x99, 0x55, 0x66]) {
        fail("no Spartan-6 sync word in file; pass the .bin, not the .bit");
    }

    let mut port = Port::open(&args.port);
    port.restart();
    port.flush_input();
    port.write(if args.flash { b"V" } else { b"R" });
    port.expect(b'R', 2, "Mojo did not respond; is this the right port?");
    port.write(&(data.len() as u32).to_le_bytes());
    port.expect(b'O', 10, "Mojo did not acknowledge the transfer size");
    let t0 = Instant::now();
    port.write(&data);
    check(unsafe { libc::tcdrain(port.0.as_raw_fd()) }, "tcdrain");
    port.expect(b'D', 10, "Mojo did not acknowledge the transfer");
    println!("sent {} bytes in {:.1} s", data.len(), t0.elapsed().as_secs_f64());

    if args.flash {
        port.write(b"S");
        port.expect(0xaa, 2, "flash does not contain a valid start byte");
        let size = port.read(4, Duration::from_secs(2));
        let size = u32::from_le_bytes(size.try_into().unwrap_or_else(|_| fail("short flash size reply"))) as usize;
        if size != data.len() + 5 {
            fail(format!("flash size mismatch ({size} != {})", data.len() + 5));
        }
        // The readback takes about as long as the upload (~18 s for an LX9
        // bitstream), so time out on silence, not on the total.
        let t1 = Instant::now();
        let back = port.read_idle(data.len(), Duration::from_secs(2));
        if let Some(bad) = back.iter().zip(&data).position(|(a, b)| a != b) {
            fail(format!("verification failed: byte {bad} of {} differs", data.len()));
        }
        if back.len() < data.len() {
            fail(format!(
                "verification incomplete: the Mojo stopped sending after {} of {} bytes ({:.1} s)",
                back.len(),
                data.len(),
                t1.elapsed().as_secs_f64()
            ));
        }
        println!("flash verified");
        port.write(b"L");
        port.expect(b'D', 5, "could not load the FPGA from flash");
    }
    println!("done: FPGA configured{}", if args.flash { " from flash" } else { " (RAM; lost on power cycle)" });
}
