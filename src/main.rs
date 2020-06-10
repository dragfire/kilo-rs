use std::io::{self, Read};
use std::os::unix::io::{AsRawFd, RawFd};
use termios::*;

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

fn disable_raw_mode(raw: Termios) -> Result<(), io::Error> {
    let mut raw = raw;
    tcsetattr(io::stdin().as_raw_fd(), TCSAFLUSH, &mut raw)?;
    Ok(())
}

fn main() {
    let mut stdin = io::stdin();
    let orig_raw = enable_raw_mode(stdin.as_raw_fd()).unwrap();

    loop {
        let byte = stdin.by_ref().bytes().next();
        let c = match byte {
            Some(ch) => ch.ok().unwrap() as char,
            None => '\0',
        };

        if c.is_ascii_control() {
            println!("{}\r", c as u8);
        } else {
            println!("{},({})\r", c as u8, c);
        }

        if c == 'q' {
            break;
        }
    }

    disable_raw_mode(orig_raw).unwrap();
}
