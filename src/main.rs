use std::env;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::process::exit;
use termios::*;

struct EditorConfig {
    cx: u16,
    cy: u16,
    screenrows: u16,
    screencols: u16,
    term: Termios,
    fd: RawFd,
}

const ESCAPE_SEQ: char = '\x1b';
const ARROW_LEFT: char = 'a';
const ARROW_RIGHT: char = 'd';
const ARROW_UP: char = 'w';
const ARROW_DOWN: char = 's';

impl Default for EditorConfig {
    fn default() -> Self {
        let fd = io::stdin().as_raw_fd();
        let mut term: Termios = Termios::from_fd(fd).unwrap();
        tcgetattr(fd, &mut term).unwrap();

        let (screenrows, screencols) = get_window_size().unwrap();

        EditorConfig {
            cx: 0,
            cy: 0,
            screenrows,
            screencols,
            term,
            fd,
        }
    }
}

fn get_window_size() -> Option<(u16, u16)> {
    let mut winsize = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let mut out = io::stdout();

    unsafe {
        if libc::ioctl(out.as_raw_fd(), libc::TIOCGWINSZ, &mut winsize) == -1 {
            if out.write(b"\x1b[999C\x1b[999B").unwrap() != 12 {
                return None;
            }

            return get_cursor_position();
        }
    }

    Some((winsize.ws_row, winsize.ws_col))
}

fn get_cursor_position() -> Option<(u16, u16)> {
    let mut out = io::stdout();
    let mut inp = io::stdin();
    if out.write(b"\x1b[6n").unwrap() != 4 {
        None
    } else {
        let mut buf: [u8; 32] = [0; 32];
        let mut i = 0;
        let inp = &mut inp;

        while i < 31 {
            let mut handle = inp.take(1);
            let mut b = [0 as u8; 1];
            if handle.read(&mut b).unwrap() != 1 {
                break;
            }
            buf[i] = b[0];
            if buf[i] as char == 'R' {
                break;
            }
            i += 1;
        }

        buf[i] = 0;

        if buf[0] as char != '\x1b' || buf[1] as char != '[' {
            return None;
        }
        Some((0, 0))
    }
}

fn ctrl_key(k: char) -> u8 {
    k as u8 & 0x1f
}

fn enable_raw_mode(cfg: &EditorConfig) -> Result<(), io::Error> {
    let mut raw = cfg.term;
    raw.c_iflag &= !(BRKINT | INPCK | ISTRIP | ICRNL | IXON);
    raw.c_oflag &= !(OPOST);
    raw.c_cflag |= CS8;
    raw.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
    raw.c_cc[VMIN] = 0;
    raw.c_cc[VTIME] = 1;
    tcsetattr(cfg.fd, TCSAFLUSH, &mut raw)?;
    Ok(())
}

fn disable_raw_mode(raw: &Termios) -> Result<(), io::Error> {
    let mut raw = raw;
    tcsetattr(io::stdin().as_raw_fd(), TCSAFLUSH, &mut raw)?;
    Ok(())
}

fn editor_read_key() -> char {
    let mut inp = io::stdin();
    let mut c = [0];
    loop {
        let read_result = inp.read(&mut c);
        if let Ok(n) = read_result {
            if n > 0 {
                break;
            }
        } else {
            term_refresh();
        }
    }
    let c = c[0] as char;

    if c == ESCAPE_SEQ {
        let mut seq = [0 as u8; 3];
        let mut handle = io::stdin().take(2);
        if handle.read(&mut seq).unwrap() != 2 {
            return ESCAPE_SEQ;
        }
        if seq[0] as char == '[' {
            return match seq[1] as char {
                'A' => ARROW_UP,
                'B' => ARROW_DOWN,
                'C' => ARROW_RIGHT,
                'D' => ARROW_LEFT,
                _ => ESCAPE_SEQ,
            };
        }
    }

    c
}

fn term_refresh() {
    let mut out = io::stdout();
    let out = out.by_ref();
    out.write(b"\x1b[2J").unwrap();
    out.write(b"\x1b[H").unwrap();
}

fn editor_draw_rows(cfg: &EditorConfig, abuf: &mut String) {
    for y in 0..cfg.screenrows {
        if y == cfg.screenrows / 3 {
            let welcome = format!("Kilo editor -- version {}", env!("CARGO_PKG_VERSION"));
            let mut welcomelen = welcome.len() as u16;
            if welcomelen > cfg.screencols {
                welcomelen = cfg.screencols;
            }
            let mut padding = (cfg.screencols - welcomelen) / 2;

            if padding > 0 {
                abuf.push('~');
                padding -= 1;
            }

            while padding > 0 {
                abuf.push(' ');
                padding -= 1;
            }
            abuf.push_str(&welcome);
        } else {
            abuf.push('~');
        }

        abuf.push_str("\x1b[K");
        if y < cfg.screenrows - 1 {
            abuf.push_str("\r\n");
        }
    }
}

fn editor_refresh_screen(cfg: &EditorConfig) {
    let mut out = io::stdout();
    let mut abuf = String::new();

    abuf.push_str("\x1b[?25l");
    abuf.push_str("\x1b[H");

    editor_draw_rows(cfg, &mut abuf);

    abuf.push_str(&format!("\x1b[{};{}H", cfg.cy + 1, cfg.cx + 1));
    abuf.push_str("\x1b[?25h");

    out.write(abuf.as_bytes()).unwrap();
    out.flush().unwrap();
}

fn editor_process_keypress(cfg: &mut EditorConfig) {
    let c = editor_read_key();
    let ctrl_q = ctrl_key('q');

    match c {
        x if x as u8 == ctrl_q => {
            term_refresh();
            disable_raw_mode(&cfg.term).unwrap();
            exit(0);
        }
        ARROW_LEFT | ARROW_RIGHT | ARROW_UP | ARROW_DOWN => {
            editor_move_cursor(cfg, c);
        }
        _ => (),
    }
}

fn editor_move_cursor(cfg: &mut EditorConfig, key: char) {
    match key {
        ARROW_LEFT => {
            print!("LEFT\r\n");
            if cfg.cx != 0 {
                cfg.cx -= 1;
            }
        }
        ARROW_RIGHT => {
            print!("RIGHT\r\n");
            if cfg.cx != cfg.screencols - 1 {
                cfg.cx += 1;
            }
        }
        ARROW_UP => {
            print!("UP\r\n");
            if cfg.cy != 0 {
                cfg.cy -= 1;
            }
        }
        ARROW_DOWN => {
            print!("DOWN\r\n");
            if cfg.cy != cfg.screenrows - 1 {
                cfg.cy += 1;
            }
        }
        _ => (),
    }
}

fn main() {
    let mut cfg = EditorConfig::default();
    enable_raw_mode(&cfg).unwrap();

    loop {
        editor_refresh_screen(&cfg);
        editor_process_keypress(&mut cfg);
        print!("Hel\r\n");
    }
}
