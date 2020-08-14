use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::process::exit;
use std::time::{Duration, SystemTime};
use termios::*;

const KILO_TAB_STOP: usize = 8;
const KILO_QUIT_TIMES: usize = 3;

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
    dirty: bool,
    quit_times: usize,
    filename: Option<String>,
    status_msg: String,
    status_msg_time: SystemTime,
}

#[derive(Eq, PartialEq)]
enum EditorKey {
    Char(char),
    Ctrl(char),
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    DeleteKey,
    PageUp,
    PageDown,
    HomeKey,
    EndKey,
    EscapeSeq,
    CarriageReturn,
    Backspace,
}

// TODO Implement `Result`, remove `unwrap`.

impl Default for EditorConfig {
    fn default() -> Self {
        let fd = io::stdin().as_raw_fd();
        let mut term: Termios = Termios::from_fd(fd).unwrap();
        tcgetattr(fd, &mut term).unwrap();

        let (mut screenrows, screencols) = get_window_size().unwrap();
        screenrows -= 2;

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
            dirty: false,
            quit_times: KILO_QUIT_TIMES,
            filename: None,
            status_msg: String::new(),
            status_msg_time: SystemTime::now(),
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

fn ctrl_key(k: char) -> char {
    (k as u8 & 0x1f) as char
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
fn editor_read_key() -> EditorKey {
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

    let c = c[0];
    let esc_seq = 0x1b;

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
                        '1' => EditorKey::HomeKey,
                        '3' => EditorKey::DeleteKey,
                        '4' => EditorKey::EndKey,
                        '5' => EditorKey::PageUp,
                        '6' => EditorKey::PageDown,
                        '7' => EditorKey::HomeKey,
                        '8' => EditorKey::EndKey,
                        _ => EditorKey::EscapeSeq,
                    };
                }
            } else {
                return match seq1_char {
                    'A' => EditorKey::ArrowUp,
                    'B' => EditorKey::ArrowDown,
                    'C' => EditorKey::ArrowRight,
                    'D' => EditorKey::ArrowLeft,
                    _ => EditorKey::EscapeSeq,
                };
            }
        } else if seq0_char == '0' {
            if seq1_char == 'H' {
                return EditorKey::HomeKey;
            } else if seq1_char == 'F' {
                return EditorKey::EndKey;
            } else {
                return EditorKey::EscapeSeq;
            }
        }
    } else if c == 127 {
        return EditorKey::Backspace;
    }
    let ch = c as char;
    if ch.is_ascii_control() {
        EditorKey::Ctrl(ch)
    } else {
        match ch {
            '\r' => EditorKey::CarriageReturn,
            _ => EditorKey::Char(ch),
        }
    }
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

fn editor_set_status_msg(cfg: &mut EditorConfig, msg: String) {
    cfg.status_msg = msg;
    cfg.status_msg_time = SystemTime::now();
}

fn editor_draw_status_bar(cfg: &EditorConfig, abuf: &mut String) {
    abuf.push_str("\x1b[7m");
    let mut status = format!(
        "{0:.20} - {1} lines",
        cfg.filename.as_ref().unwrap_or(&"[No Name]".to_string()),
        cfg.numrows
    );

    if cfg.dirty {
        status.push_str("(modified)");
    }

    let rstatus = format!("{}/{}", cfg.cy + 1, cfg.numrows);
    let rlen = rstatus.len();

    let mut len = status.len();
    if len > cfg.screencols {
        len = cfg.screencols;
    }
    abuf.push_str(&status);

    for i in len..cfg.screencols {
        if cfg.screencols - i == rlen {
            abuf.push_str(&rstatus);
            break;
        } else {
            abuf.push_str(" ");
        }
    }

    abuf.push_str("\x1b[m");
    abuf.push_str("\r\n");
}

fn editor_draw_message_bar(cfg: &EditorConfig, abuf: &mut String) {
    abuf.push_str("\x1b[K");
    let mut len = cfg.status_msg.len();
    if len > cfg.screencols {
        len = cfg.screencols;
    }
    if len > 0 && cfg.status_msg_time.elapsed().unwrap() < Duration::from_secs(5) {
        abuf.push_str(&cfg.status_msg);
    }
}

fn editor_refresh_screen(cfg: &mut EditorConfig) {
    editor_scroll(cfg);

    let mut out = io::stdout();
    let mut abuf = String::new();

    abuf.push_str("\x1b[?25l");
    abuf.push_str("\x1b[H");

    editor_draw_rows(cfg, &mut abuf);
    editor_draw_status_bar(cfg, &mut abuf);
    editor_draw_message_bar(cfg, &mut abuf);

    abuf.push_str(&format!(
        "\x1b[{};{}H",
        (cfg.cy - cfg.rowoff) + 1,
        (cfg.rx - cfg.coloff) + 1
    ));
    abuf.push_str("\x1b[?25h");

    out.write(abuf.as_bytes()).unwrap();
    out.flush().unwrap();
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

fn editor_append_row(cfg: &mut EditorConfig, chars: String) {
    let mut row = Row::default();
    row.chars = chars.to_string();
    row.render = chars;
    editor_update_row(&mut row);
    cfg.rows.push(row);
    cfg.dirty = true;
}

fn editor_update_row(row: &mut Row) {
    let mut idx = 0;
    row.render.clear();

    for c in row.chars.chars() {
        if c == '\t' {
            row.render.push(' ');
            idx += 1;

            while idx % KILO_TAB_STOP != 0 {
                row.render.push(' ');
                idx += 1;
            }
        } else {
            row.render.push(c);
        }
    }
}

fn editor_row_insert_char(row: &mut Row, mut at: usize, c: char) {
    if at > row.chars.len() {
        at = row.chars.len();
    }
    row.chars.insert(at, c);
    editor_update_row(row);
}

fn editor_insert_char(cfg: &mut EditorConfig, c: char) {
    if cfg.cy == cfg.numrows {
        editor_append_row(cfg, String::new());
    }
    editor_row_insert_char(&mut cfg.rows[cfg.cy], cfg.cx, c);
    cfg.cx += 1;
    cfg.dirty = true;
}

fn editor_row_del_char(row: &mut Row, at: usize) {
    if at >= row.chars.len() {
        return;
    }
    row.chars.remove(at);
    editor_update_row(row);
}

fn editor_del_char(cfg: &mut EditorConfig) {
    if cfg.cy == cfg.numrows {
        return;
    }
    if cfg.cx > 0 {
        editor_row_del_char(&mut cfg.rows[cfg.cy], cfg.cx - 1);
        cfg.cx -= 1;
    }
    cfg.dirty = true;
}

fn editor_process_keypress(cfg: &mut EditorConfig) {
    let c = editor_read_key();

    match c {
        EditorKey::ArrowUp
        | EditorKey::ArrowDown
        | EditorKey::ArrowLeft
        | EditorKey::ArrowRight => {
            editor_move_cursor(cfg, c);
        }
        EditorKey::PageUp | EditorKey::PageDown => {
            if c == EditorKey::PageUp {
                cfg.cy = cfg.rowoff;
            } else if c == EditorKey::PageDown {
                cfg.cy = cfg.rowoff + cfg.screenrows - 1;
                if cfg.cy > cfg.numrows {
                    cfg.cy = cfg.numrows;
                }
            }

            for _ in 0..cfg.screenrows {
                let key = if c == EditorKey::PageUp {
                    EditorKey::ArrowUp
                } else {
                    EditorKey::ArrowDown
                };

                editor_move_cursor(cfg, key);
            }
        }
        EditorKey::HomeKey => {
            cfg.cx = 0;
        }
        EditorKey::EndKey => {
            if cfg.cy < cfg.numrows {
                cfg.cx = cfg.rows[cfg.cy].chars.len();
            }
        }
        EditorKey::Ctrl(c) => {
            if c == ctrl_key('q') {
                if cfg.dirty && cfg.quit_times > 0 {
                    editor_set_status_msg(
                        cfg,
                        format!(
                            "WARNING!!! File has unsaved changes. \
                            Press Ctrl-Q {} more times to quit.",
                            cfg.quit_times,
                        ),
                    );
                    cfg.quit_times -= 1;
                    return;
                }
                term_refresh();
                disable_raw_mode(&cfg.term).unwrap();
                exit(0);
            } else if c == ctrl_key('s') {
                editor_save(cfg);
            }
        }
        EditorKey::Char(c) => {
            editor_insert_char(cfg, c);
        }
        EditorKey::DeleteKey | EditorKey::Backspace => {
            editor_move_cursor(cfg, EditorKey::ArrowRight);
            editor_del_char(cfg);
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

// *** File I/O ***

fn editor_open(cfg: &mut EditorConfig, filename: &str) {
    cfg.filename = Some(filename.to_string());
    let file = File::open(filename).unwrap();
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = line.unwrap();
        editor_append_row(cfg, line);
    }

    cfg.numrows = cfg.rows.len();
    cfg.dirty = false;
}

fn editor_rows_to_string(cfg: &EditorConfig) -> String {
    let mut buf = String::new();
    for row in &cfg.rows {
        buf.push_str(&row.chars);
        buf.push('\n');
    }

    buf
}

fn editor_save(cfg: &mut EditorConfig) {
    if let Some(filename) = cfg.filename.as_ref() {
        let buf = editor_rows_to_string(cfg);
        let mut fd = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(filename)
            .unwrap();

        match fd.write(buf.as_bytes()) {
            Ok(n) => {
                cfg.dirty = false;
                editor_set_status_msg(cfg, format!("{} bytes written to disk", n));
            }
            Err(e) => editor_set_status_msg(cfg, format!("Can't save I/O error: {}", e)),
        }

        if let Err(e) = fd.flush() {
            editor_set_status_msg(cfg, format!("Can't save I/O error: {}", e));
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

    editor_set_status_msg(&mut cfg, "HELP: Ctrl-S = save | Ctrl-Q = quit".to_string());

    loop {
        editor_refresh_screen(&mut cfg);
        editor_process_keypress(&mut cfg);
    }
}
