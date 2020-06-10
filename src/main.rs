use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::process::exit;
use termios::*;

fn ctrl_key(k: char) -> u8 {
    k as u8 & 0x1f
}

fn enable_raw_mode(fd: RawFd) -> Result<Termios, io::Error> {
    let mut raw: Termios = Termios::from_fd(fd)?;
    let orig_raw = raw;
    tcgetattr(fd, &mut raw)?;
    raw.c_iflag &= !(BRKINT | INPCK | ISTRIP | ICRNL | IXON);
    raw.c_oflag &= !(OPOST);
    raw.c_cflag |= CS8;
    raw.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
    raw.c_cc[VMIN] = 0;
    raw.c_cc[VTIME] = 1;
    tcsetattr(fd, TCSAFLUSH, &mut raw)?;
    Ok(orig_raw)
}

fn disable_raw_mode(raw: &Termios) -> Result<(), io::Error> {
    let mut raw = raw;
    tcsetattr(io::stdin().as_raw_fd(), TCSAFLUSH, &mut raw)?;
    Ok(())
}

fn editor_read_key() -> char {
    let byte = io::stdin().by_ref().bytes().next();

    let c = match byte {
        Some(ch) => ch.ok().unwrap() as char,
        None => '\0',
    };

    c
}

fn editor_refresh_screen() {
    let mut out = io::stdout();
    let out = out.by_ref();
    out.write(b"\x1b[2J").unwrap();
    out.write(b"\x1b[H").unwrap();
}

fn editor_process_keypress(raw: &Termios) {
    let c = editor_read_key();
    let ctrl_q = ctrl_key('q');
    match c as u8 {
        x if x == ctrl_q => {
            editor_refresh_screen();
            disable_raw_mode(raw).unwrap();
            exit(0);
        }
        _ => (),
    }
}

fn main() {
    let raw = enable_raw_mode(io::stdin().as_raw_fd()).unwrap();
    loop {
        editor_refresh_screen();
        editor_process_keypress(&raw);
    }
}
