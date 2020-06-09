use std::io::{self, Read};
use std::os::unix::io::{AsRawFd, RawFd};
use std::process::exit;
use termios::*;

fn enable_raw_mode(fd: RawFd) -> Result<(), io::Error> {
    let mut raw: Termios = Termios::from_fd(fd)?;
    tcgetattr(fd, &mut raw)?;
    raw.c_lflag &= !(ECHO);
    tcsetattr(fd, TCSAFLUSH, &mut raw)?;
    Ok(())
}

fn main() {
    let mut stdin = io::stdin();
    enable_raw_mode(stdin.as_raw_fd()).unwrap();
    while let Some(b) = stdin.by_ref().bytes().next() {
        let c = b.ok().unwrap() as char;
        if c == 'q' {
            exit(0);
        }
    }
}
