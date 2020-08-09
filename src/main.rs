use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::process::exit;
use termios::*;

const KILO_TAB_STOP: usize = 8;

#[derive(Default)]
struct Row {
    chars: String,
    render: String,
}

struct EditorConfig {
    cx: usize,
    cy: usize,
    rx: usize,
    screenrows: usize,
    screencols: usize,
    rowoff: usize,
    coloff: usize,
    numrows: usize,
    rows: Vec<Row>,
    term: Termios,
    fd: RawFd,
    filename: String,
}

#[derive(Eq, PartialEq)]
enum EditorKey {
    ArrowLeft = 1000,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    DeleteKey,
    PageUp,
    PageDown,
    HomeKey,
    EndKey,
    EscapeSeq = 0x1b,
}

impl From<EditorKey> for usize {
    fn from(key: EditorKey) -> Self {
        match key {
            EditorKey::ArrowLeft => 1000,
            EditorKey::ArrowRight => 1001,
            EditorKey::ArrowUp => 1002,
            EditorKey::ArrowDown => 1003,
            EditorKey::DeleteKey => 1004,
            EditorKey::PageUp => 1005,
            EditorKey::PageDown => 1006,
            EditorKey::HomeKey => 1007,
            EditorKey::EndKey => 1008,
            EditorKey::EscapeSeq => 0x1b,
        }
    }
}

impl From<usize> for EditorKey {
    fn from(key: usize) -> Self {
        match key {
            1000 => EditorKey::ArrowLeft,
            1001 => EditorKey::ArrowRight,
            1002 => EditorKey::ArrowUp,
            1003 => EditorKey::ArrowDown,
            1004 => EditorKey::DeleteKey,
            1005 => EditorKey::PageUp,
            1006 => EditorKey::PageDown,
            1007 => EditorKey::HomeKey,
            1008 => EditorKey::EndKey,
            0x1b => EditorKey::EscapeSeq,
            _ => EditorKey::EscapeSeq,
        }
    }
}

// TODO Implement `Result`, remove `unwrap`.

impl Default for EditorConfig {
    fn default() -> Self {
        let fd = io::stdin().as_raw_fd();
        let mut term: Termios = Termios::from_fd(fd).unwrap();
        tcgetattr(fd, &mut term).unwrap();

        let (mut screenrows, screencols) = get_window_size().unwrap();
        screenrows -= 1;

        EditorConfig {
            cx: 0,
            cy: 0,
            rx: 0,
            fd,
            term,
            rowoff: 0,
            coloff: 0,
            rows: Vec::new(),
            numrows: 0,
            screenrows,
            screencols,
            filename: String::new(),
        }
    }
}

fn get_window_size() -> Option<(usize, usize)> {
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

    Some((winsize.ws_row as usize, winsize.ws_col as usize))
}

fn get_cursor_position() -> Option<(usize, usize)> {
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

fn ctrl_key(k: char) -> usize {
    (k as u8 & 0x1f) as usize
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

/// Read a key and wait for the next one.
///
/// It also handles keys with Escape sequences.
fn editor_read_key() -> usize {
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
            exit(1);
        }
    }

    let c = c[0] as usize;
    let esc_seq = EditorKey::EscapeSeq.into();

    if c == esc_seq {
        let mut seq = [0 as u8; 3];
        let mut handle = io::stdin().take(3);
        handle.read(&mut seq).unwrap();
        let seq0_char = seq[0] as char;
        let seq1_char = seq[1] as char;
        if seq0_char == '[' {
            if seq[1] >= '0' as u8 && seq[1] <= '9' as u8 {
                if seq[2] as char == '~' {
                    return match seq1_char {
                        '1' => EditorKey::HomeKey.into(),
                        '3' => EditorKey::DeleteKey.into(),
                        '4' => EditorKey::EndKey.into(),
                        '5' => EditorKey::PageUp.into(),
                        '6' => EditorKey::PageDown.into(),
                        '7' => EditorKey::HomeKey.into(),
                        '8' => EditorKey::EndKey.into(),
                        _ => esc_seq,
                    };
                }
            } else {
                return match seq1_char {
                    'A' => EditorKey::ArrowUp.into(),
                    'B' => EditorKey::ArrowDown.into(),
                    'C' => EditorKey::ArrowRight.into(),
                    'D' => EditorKey::ArrowLeft.into(),
                    _ => esc_seq,
                };
            }
        } else if seq0_char == '0' {
            if seq1_char == 'H' {
                return EditorKey::HomeKey.into();
            } else if seq1_char == 'F' {
                return EditorKey::EndKey.into();
            } else {
                return esc_seq;
            }
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

fn editor_scroll(cfg: &mut EditorConfig) {
    cfg.rx = 0;
    if cfg.cy < cfg.numrows {
        cfg.rx = editor_row_cx_to_rx(&cfg.rows[cfg.cy], cfg.cx);
    }

    if cfg.cy < cfg.rowoff {
        cfg.rowoff = cfg.cy;
    }
    if cfg.cy >= cfg.rowoff + cfg.screenrows {
        cfg.rowoff = cfg.cy - cfg.screenrows + 1;
    }
    if cfg.rx < cfg.coloff {
        cfg.coloff = cfg.rx;
    }
    if cfg.rx >= cfg.coloff + cfg.screencols {
        cfg.coloff = cfg.rx - cfg.screencols + 1;
    }
}

fn editor_draw_rows(cfg: &EditorConfig, abuf: &mut String) {
    for y in 0..cfg.screenrows {
        let filerow = y + cfg.rowoff;
        if filerow >= cfg.numrows {
            if cfg.numrows == 0 && y == cfg.screenrows / 3 {
                let welcome = format!("Kilo editor -- version {}", env!("CARGO_PKG_VERSION"));
                let mut welcomelen = welcome.len();
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
        } else {
            let rows = &cfg.rows;

            // since I am using usize, need to avoid overflow error
            // when length of a row is less than coloff.
            let mut len = rows[filerow].render.len().saturating_sub(cfg.coloff);
            if len > cfg.screencols {
                len = cfg.screencols;
            }

            let mut slice = rows[filerow].render.as_str();
            if cfg.coloff < cfg.coloff + len {
                slice = &slice[cfg.coloff..cfg.coloff + len];
            } else {
                slice = "";
            }
            abuf.push_str(slice);
        }

        abuf.push_str("\x1b[K");
        abuf.push_str("\r\n");
    }
}

fn editor_draw_status_bar(cfg: &EditorConfig, abuf: &mut String) {
    abuf.push_str("\x1b[7m");
    let status = String::new();

    for _ in 0..cfg.screencols {
        abuf.push_str(" ");
    }
    abuf.push_str("\x1b[m");
}

fn editor_refresh_screen(cfg: &mut EditorConfig) {
    editor_scroll(cfg);

    let mut out = io::stdout();
    let mut abuf = String::new();

    abuf.push_str("\x1b[?25l");
    abuf.push_str("\x1b[H");

    editor_draw_rows(cfg, &mut abuf);
    editor_draw_status_bar(cfg, &mut abuf);

    abuf.push_str(&format!(
        "\x1b[{};{}H",
        (cfg.cy - cfg.rowoff) + 1,
        (cfg.rx - cfg.coloff) + 1
    ));
    abuf.push_str("\x1b[?25h");

    out.write(abuf.as_bytes()).unwrap();
    out.flush().unwrap();
}

fn editor_process_keypress(cfg: &mut EditorConfig) {
    let c = editor_read_key();
    let ctrl_q = ctrl_key('q');

    match c {
        x if x == ctrl_q => {
            term_refresh();
            disable_raw_mode(&cfg.term).unwrap();
            exit(0);
        }
        1000..=1004 => {
            editor_move_cursor(cfg, c.into());
        }
        1005 | 1006 => {
            let key: EditorKey = c.into();
            if key == EditorKey::PageUp {
                cfg.cy = cfg.rowoff;
            } else if key == EditorKey::PageDown {
                cfg.cy = cfg.rowoff + cfg.screenrows - 1;
                if cfg.cy > cfg.numrows {
                    cfg.cy = cfg.numrows;
                }
            }

            for _ in 0..cfg.screenrows {
                let key = if key == EditorKey::PageUp {
                    EditorKey::ArrowUp
                } else {
                    EditorKey::ArrowDown
                };

                editor_move_cursor(cfg, key);
            }
        }
        1007 => {
            cfg.cx = 0;
        }
        1008 => {
            if cfg.cy < cfg.numrows {
                cfg.cx = cfg.rows[cfg.cy].chars.len();
            }
        }
        _ => (),
    }
}

fn editor_move_cursor(cfg: &mut EditorConfig, key: EditorKey) {
    let mut row = &Row::default();
    if cfg.cy < cfg.numrows {
        row = &cfg.rows[cfg.cy];
    }
    match key {
        EditorKey::ArrowLeft => {
            if cfg.cx != 0 {
                cfg.cx -= 1;
            } else if cfg.cy > 0 {
                cfg.cy -= 1;
                cfg.cx = cfg.rows[cfg.cy].chars.len();
            }
        }
        EditorKey::ArrowRight => {
            if cfg.cx < row.chars.len() {
                cfg.cx += 1;
            } else if cfg.cx == row.chars.len() {
                cfg.cy += 1;
                cfg.cx = 0;
            }
        }
        EditorKey::ArrowUp => {
            if cfg.cy != 0 {
                cfg.cy -= 1;
            }
        }
        EditorKey::ArrowDown => {
            if cfg.cy < cfg.numrows {
                cfg.cy += 1;
            }
        }
        EditorKey::DeleteKey => unimplemented!("DeleteKey"),
        _ => (),
    }
    if cfg.cy < cfg.numrows {
        row = &cfg.rows[cfg.cy];
    }

    let rowlen = row.chars.len();
    if cfg.cx > rowlen {
        cfg.cx = rowlen;
    }
}

fn editor_row_cx_to_rx(row: &Row, cx: usize) -> usize {
    let mut rx = 0;
    let slice = &row.chars[..cx];
    for c in slice.chars() {
        if c == '\t' {
            rx += (KILO_TAB_STOP - 1) - (rx % KILO_TAB_STOP);
        }
        rx += 1;
    }

    rx
}

fn editor_open(cfg: &mut EditorConfig, filename: &str) {
    cfg.filename = filename.to_string();
    let file = std::fs::File::open(filename).unwrap();
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = line.unwrap();
        let mut row = Row::default();
        row.chars = line.to_string();
        row.render = line;
        editor_update_row(&mut row);
        cfg.rows.push(row);
    }

    cfg.numrows = cfg.rows.len();
}

fn editor_update_row(row: &mut Row) {
    let mut idx = 0;

    for c in row.chars.chars() {
        if c == '\t' {
            row.render.push(' ');
            idx += 1;

            while idx % KILO_TAB_STOP != 0 {
                row.render.push(' ');
                idx += 1;
            }
        }
    }
}

fn main() {
    let args = std::env::args().collect::<Vec<String>>();
    let mut cfg = EditorConfig::default();
    enable_raw_mode(&cfg).unwrap();

    if args.len() > 1 {
        let filename = &args[1];
        editor_open(&mut cfg, filename);
    }

    loop {
        editor_refresh_screen(&mut cfg);
        editor_process_keypress(&mut cfg);
    }
}
