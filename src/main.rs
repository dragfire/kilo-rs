use std::collections::HashSet;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::iter::FromIterator;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::Path;
use std::process::exit;
use std::time::{Duration, SystemTime};
use termios::*;

// TODO Implement `Result`, remove `unwrap`.

const KILO_TAB_STOP: usize = 8;
const KILO_QUIT_TIMES: usize = 3;

/// Row stores information about characters in a row
///
/// Supports rendering tabs or spaces and syntax highlighting.
#[derive(Default)]
struct Row {
    idx: usize,
    chars: String,
    render: String,
    hl: Vec<Highlight>,
    hl_open_comment: bool,
}

#[derive(Eq, PartialEq)]
enum Direction {
    Forward,
    Backward,
}

#[derive(Eq, PartialEq, Clone, Copy)]
enum Highlight {
    Normal,
    Comment,
    MLComment,
    Keyword1,
    Keyword2,
    String,
    Number,
    Match,
}

impl From<Highlight> for i32 {
    fn from(hl: Highlight) -> i32 {
        match hl {
            Highlight::Comment | Highlight::MLComment => 36,
            Highlight::Keyword1 => 33,
            Highlight::Keyword2 => 32,
            Highlight::String => 35,
            Highlight::Number => 31,
            Highlight::Match => 34,
            _ => 37,
        }
    }
}

/// Set bit flags
enum HighlightFlag {
    Number = 1 << 0, // 01
    String = 1 << 1, // 10
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
    last_match: isize,
    direction: Direction,
    saved_hl_line: isize,
    saved_hl: Option<Vec<Highlight>>,
    hldb: Vec<EditorSyntax>,
    editor_syntax: Option<EditorSyntax>,
}

impl EditorConfig {
    fn new() -> Self {
        let fd = io::stdin().as_raw_fd();
        let mut term: Termios = Termios::from_fd(fd).unwrap();
        tcgetattr(fd, &mut term).unwrap();

        let (mut screenrows, screencols) = get_window_size().unwrap();
        screenrows -= 2;

        let c_filematch = vec!["c".to_string(), "h".to_string(), "cpp".to_string()];
        let c_keywords: Vec<String> = vec![
            "switch",
            "if",
            "while",
            "for",
            "break",
            "continue",
            "return",
            "else",
            "struct",
            "union",
            "typedef",
            "static",
            "enum",
            "class",
            "case",
            "int|",
            "long|",
            "double|",
            "float|",
            "char|",
            "unsigned|",
            "signed|",
            "void|",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let mut hldb = Vec::new();
        hldb.push(EditorSyntax::new(
            "c",
            HashSet::from_iter(c_filematch),
            c_keywords,
            "//".to_string(),
            "/*".to_string(),
            "*/".to_string(),
            HighlightFlag::Number as u8 | HighlightFlag::String as u8,
        ));

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
            last_match: -1,
            direction: Direction::Forward,
            saved_hl_line: -1,
            saved_hl: None,
            hldb,
            editor_syntax: None,
        }
    }
}

/// EditorKey represents all Keys pressed
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

#[derive(Clone)]
struct EditorSyntax {
    filetype: String,
    filematch: HashSet<String>,
    keywords: Vec<String>,
    singleline_comment_start: String,
    multiline_comment_start: String,
    multiline_comment_end: String,
    flags: u8,
}

impl EditorSyntax {
    fn new(
        filetype: &str,
        filematch: HashSet<String>,
        keywords: Vec<String>,
        singleline_comment_start: String,
        multiline_comment_start: String,
        multiline_comment_end: String,
        flags: u8,
    ) -> Self {
        EditorSyntax {
            filetype: filetype.to_string(),
            filematch,
            keywords,
            singleline_comment_start,
            multiline_comment_start,
            multiline_comment_end,
            flags,
        }
    }
}

// *** Terminal ***

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

// *** Syntax Highlighting ***

fn editor_update_syntax(edit_syntax: Option<&EditorSyntax>, rows: &mut [Row], cy: usize) {
    let rows = std::cell::RefCell::new(rows);
    let numrows = rows.borrow().len();
    let mut brows = rows.borrow_mut();
    let (left, right) = brows.split_at_mut(cy);
    let row = &mut right[0];
    let prev_row = left.last();
    row.hl = vec![Highlight::Normal; row.chars.len()];

    if let Some(syntax) = edit_syntax {
        let mut in_comment = row.idx > 0 && prev_row.map(|r| r.hl_open_comment).unwrap_or(false);
        apply_syntax(syntax, in_comment, row);
        let mut changed = row.hl_open_comment != in_comment;
        row.hl_open_comment = in_comment;
        while changed && row.idx + 1 < numrows {
            let mut brows = rows.borrow_mut();
            let (left, right) = brows.split_at_mut(row.idx + 1);
            let row = &mut right[0];
            let prev_row = left.last();
            in_comment = row.idx > 0 && prev_row.map(|r| r.hl_open_comment).unwrap_or(true);
            apply_syntax(syntax, in_comment, row);
            changed = row.hl_open_comment != in_comment;
            row.hl_open_comment = in_comment;
        }
    }
}

fn apply_syntax(syntax: &EditorSyntax, mut in_comment: bool, row: &mut Row) {
    let n = row.render.len();
    let mut prev_sep = true;
    let mut in_string = false;
    let flags = syntax.flags;

    let scs = &syntax.singleline_comment_start;
    let mcs = &syntax.multiline_comment_start;
    let mce = &syntax.multiline_comment_end;

    let scs_len = scs.len();
    let mcs_len = mcs.len();
    let mce_len = mce.len();
    let keywords = &syntax.keywords;

    let row_render_slice = row.render.as_bytes();
    let mut i = 0;
    while i < n {
        let slice = &row_render_slice[i..];
        let c = row_render_slice[i] as char;
        let prev_hl = if i > 0 {
            row.hl[i - 1]
        } else {
            Highlight::Normal
        };

        if scs_len > 0 && !in_string && !in_comment {
            if slice.starts_with(scs.as_bytes()) {
                let slice = &mut row.hl[i..];
                for el in slice {
                    *el = Highlight::Comment;
                }
                break;
            }
        }

        if mcs_len > 0 && mce_len > 0 && !in_string {
            if in_comment {
                row.hl[i] = Highlight::MLComment;
                if row.render.starts_with(mce) {
                    let slice = &mut row.hl[i..i + mce_len];
                    for el in slice {
                        *el = Highlight::MLComment;
                    }
                    i += mce_len;
                    in_comment = false;
                    prev_sep = true;
                    continue;
                } else {
                    i += 1;
                    continue;
                }
            } else if slice.starts_with(mcs.as_bytes()) {
                let slice = &mut row.hl[i..i + mcs_len];
                for el in slice {
                    *el = Highlight::MLComment;
                }
                i += mcs_len;
                in_comment = true;
                continue;
            }
        }

        if syntax.flags & HighlightFlag::String as u8 == HighlightFlag::String as u8 {
            if in_string {
                row.hl[i] = Highlight::String;
                if c == '\\' && i + 1 < n {
                    row.hl[i + 1] = Highlight::String;
                    i += 2;
                    continue;
                }
                if c == '"' || c == '\'' {
                    in_string = false;
                }
                i += 1;
                prev_sep = true;
                continue;
            } else {
                if c == '"' || c == '\'' {
                    in_string = true;
                    row.hl[i] = Highlight::String;
                    i += 1;
                    continue;
                }
            }
        }

        if flags & HighlightFlag::Number as u8 == HighlightFlag::Number as u8 {
            if c.is_ascii_digit() && (prev_sep || prev_hl == Highlight::Number)
                || (c == '.' && prev_hl == Highlight::Number)
            {
                row.hl[i] = Highlight::Number;
                i += 1;
                prev_sep = false;
                continue;
            }
        }

        if prev_sep {
            for keyword in keywords.iter() {
                let mut kw = keyword.as_str();
                let mut klen = keyword.len();
                let kw2 = keyword.get(klen - 1..);
                let mut is_kw2 = false;
                if let Some(kw2) = kw2 {
                    if kw2 == "|" {
                        klen -= 1;
                        kw = keyword.get(..klen - 1).unwrap();
                        is_kw2 = true;
                    }
                }

                let slice = &row.render[i..];
                let bytes = slice.as_bytes();
                if slice.starts_with(kw) && is_seperator(bytes[klen] as char) {
                    let slice = &mut row.hl[i..i + klen];
                    for el in slice {
                        *el = if is_kw2 {
                            Highlight::Keyword2
                        } else {
                            Highlight::Keyword1
                        };
                    }
                    i += klen;
                    break;
                }
            }
        }

        prev_sep = is_seperator(c);
        i += 1;
    }
}

/// Check if a character is a seperator character
fn is_seperator(c: char) -> bool {
    c.is_ascii_whitespace() || ",.()+-/*=~%<>[];".contains(c)
}

fn editor_select_syntax_highlight(cfg: &mut EditorConfig) {
    cfg.editor_syntax = None;
    if cfg.filename.is_none() {
        return;
    }

    let ext = Path::new(cfg.filename.as_ref().unwrap()).extension();
    if let Some(ext) = ext {
        for syntax in cfg.hldb.iter() {
            if syntax.filematch.contains(ext.to_str().unwrap()) {
                cfg.editor_syntax = Some(syntax.clone());
                for _ in 0..cfg.numrows {
                    editor_update_syntax(
                        cfg.editor_syntax.as_ref(),
                        cfg.rows.as_mut_slice(),
                        cfg.cy,
                    );
                }
                return;
            }
        }
    }
}

// *** Row Operations ***

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

fn editor_row_rx_to_cx(row: &Row, rx: usize) -> usize {
    let mut cur_rx = 0;
    let n = row.chars.len();
    let slice = &row.chars[..n];
    for (cx, c) in slice.chars().enumerate() {
        if c == '\t' {
            cur_rx += (KILO_TAB_STOP - 1) - (cur_rx % KILO_TAB_STOP);
        }
        cur_rx += 1;

        if cur_rx > rx {
            return cx;
        }
    }

    n
}

fn editor_insert_row(cfg: &mut EditorConfig, chars: String, at: usize) {
    if at > cfg.numrows {
        return;
    }

    for j in at + 1..cfg.numrows {
        cfg.rows[j].idx += 1;
    }

    let mut row = Row::default();
    row.idx = at;
    row.chars = chars;
    row.hl_open_comment = false;
    cfg.rows.insert(at, row);
    cfg.numrows = cfg.rows.len();
    cfg.dirty = true;
    editor_update_row(cfg.editor_syntax.as_ref(), cfg.rows.as_mut_slice(), at);
}

fn editor_update_row(syntax: Option<&EditorSyntax>, rows: &mut [Row], cy: usize) {
    let mut idx = 0;
    let row = &mut rows[cy];
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

    editor_update_syntax(syntax, rows, cy);
}

fn editor_free_row(row: &mut Row) {
    row.chars.clear();
    row.render.clear();
    row.hl.clear();
}

fn editor_del_row(cfg: &mut EditorConfig, at: usize) {
    if at >= cfg.numrows {
        return;
    }
    editor_free_row(&mut cfg.rows[cfg.cy]);
    cfg.rows.remove(at);
    cfg.numrows -= 1;
    for j in at..cfg.numrows - 1 {
        cfg.rows[j].idx -= 1;
    }
    cfg.dirty = true;
}

fn editor_row_insert_char(
    syntax: Option<&EditorSyntax>,
    rows: &mut [Row],
    mut at: usize,
    c: char,
    cy: usize,
) {
    let row = &mut rows[cy];
    if at > row.chars.len() {
        at = row.chars.len();
    }
    row.chars.insert(at, c);
    editor_update_row(syntax, rows, cy);
}

fn editor_row_del_char(syntax: Option<&EditorSyntax>, rows: &mut [Row], at: usize, cy: usize) {
    let row = &mut rows[cy];
    if at >= row.chars.len() {
        return;
    }
    row.chars.remove(at);
    editor_update_row(syntax, rows, cy);
}

fn editor_row_append_str(syntax: Option<&EditorSyntax>, rows: &mut [Row], cy: usize, s: &str) {
    let row = &mut rows[cy];
    row.chars.push_str(s);
    editor_update_row(syntax, rows, cy);
}

// *** Editor operations ***

fn editor_insert_char(cfg: &mut EditorConfig, c: char) {
    if cfg.cy == cfg.numrows {
        editor_insert_row(cfg, String::new(), 0);
    }
    editor_row_insert_char(
        cfg.editor_syntax.as_ref(),
        cfg.rows.as_mut_slice(),
        cfg.cx,
        c,
        cfg.cy,
    );

    cfg.cx += 1;
    cfg.dirty = true;
}

fn editor_insert_new_line(cfg: &mut EditorConfig) {
    if cfg.cx == 0 {
        editor_insert_row(cfg, String::new(), 0);
    } else {
        let chars = cfg.rows[cfg.cy].chars.to_string();
        editor_insert_row(cfg, String::from(&chars[cfg.cx - 1..]), cfg.cy + 1);

        let row = &mut cfg.rows[cfg.cy];
        row.chars = String::from(&chars[..cfg.cx - 1]);
        editor_update_row(cfg.editor_syntax.as_ref(), cfg.rows.as_mut_slice(), cfg.cy);
    }
    cfg.cy += 1;
    cfg.cx = 0;
}

fn editor_del_char(cfg: &mut EditorConfig) {
    if cfg.cy == cfg.numrows {
        return;
    }
    if cfg.cx == 0 && cfg.cy == 0 {
        return;
    }

    if cfg.cx > 0 {
        editor_row_del_char(
            cfg.editor_syntax.as_ref(),
            cfg.rows.as_mut_slice(),
            cfg.cx - 1,
            cfg.cy,
        );
        cfg.cx -= 1;
    } else {
        let chars = &cfg.rows[cfg.cy].chars.to_string();
        cfg.cx = cfg.rows[cfg.cy - 1].chars.len();
        editor_row_append_str(
            cfg.editor_syntax.as_ref(),
            cfg.rows.as_mut_slice(),
            cfg.cy - 1,
            chars,
        );
        editor_del_row(cfg, cfg.cy);
        cfg.cy -= 1;
    }
    cfg.dirty = true;
}

// *** Find ***

fn editor_find_callback(cfg: &mut EditorConfig, query: &str, key: EditorKey) {
    if let Some(ref saved_hl) = cfg.saved_hl {
        let row = &mut cfg.rows[cfg.saved_hl_line as usize];
        let original_hl = &mut row.hl;
        for (i, el) in original_hl.iter_mut().enumerate() {
            *el = saved_hl[i];
        }
    }
    match key {
        EditorKey::CarriageReturn | EditorKey::EscapeSeq => {
            cfg.last_match = -1;
            cfg.direction = Direction::Forward;
            return;
        }
        EditorKey::ArrowRight | EditorKey::ArrowDown => {
            cfg.direction = Direction::Forward;
        }
        EditorKey::ArrowLeft | EditorKey::ArrowUp => {
            cfg.direction = Direction::Backward;
        }
        _ => {
            cfg.last_match = -1;
            cfg.direction = Direction::Forward;
        }
    }

    if cfg.last_match == -1 {
        cfg.direction = Direction::Forward;
    }

    let mut current = cfg.last_match;
    for _ in 0..cfg.numrows {
        match cfg.direction {
            Direction::Forward => {
                current += 1;
            }
            Direction::Backward => {
                current -= 1;
            }
        }
        if current == -1 {
            current = (cfg.numrows - 1) as isize;
        } else if current == cfg.numrows as isize {
            current = 0;
        }

        let row = &mut cfg.rows[current as usize];
        let match_index = row.render.find(query);
        if let Some(index) = match_index {
            cfg.last_match = current;
            cfg.cy = current as usize;
            cfg.cx = editor_row_rx_to_cx(row, index);
            cfg.rowoff = cfg.numrows;

            cfg.saved_hl_line = current;
            cfg.saved_hl = Some(row.hl.clone());

            let slice = &mut row.hl[index..index + query.len()];
            for el in slice {
                *el = Highlight::Match;
            }

            break;
        }
    }
}

fn editor_find(cfg: &mut EditorConfig) {
    let saved_cx = cfg.cx;
    let saved_cy = cfg.cy;
    let saved_coloff = cfg.coloff;
    let saved_rowoff = cfg.rowoff;

    editor_prompt(
        cfg,
        |buf| format!("Search: {} (Use ESC/Arrows/Enter)", buf),
        Some(editor_find_callback),
    );

    cfg.cx = saved_cx;
    cfg.cy = saved_cy;
    cfg.coloff = saved_coloff;
    cfg.rowoff = saved_rowoff;
}

// *** Output ***

/// Clear screen and move cursor to top of the screen.
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
            let row = &cfg.rows[filerow];

            // since I am using usize, need to avoid overflow error
            // when length of a row is less than coloff.
            let mut len = row.render.len().saturating_sub(cfg.coloff);
            if len > cfg.screencols {
                len = cfg.screencols;
            }

            let slice = row.render.get(cfg.coloff..).unwrap();
            let hl = &row.hl;
            let mut curr_color: i32 = -1;

            for (i, c) in slice.chars().enumerate() {
                if i == len {
                    break;
                }

                if hl[i] == Highlight::Normal {
                    if curr_color != -1 {
                        abuf.push_str("\x1b[39m");
                        curr_color = -1;
                    }
                } else {
                    let color: i32 = hl[i].into();
                    if color != curr_color {
                        curr_color = color;
                        abuf.push_str(&format!("\x1b[{}m", color));
                    }
                }
                abuf.push(c);
            }
            abuf.push_str("\x1b[39m");
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

    let rstatus = format!(
        "{} | {}/{}",
        cfg.editor_syntax
            .as_ref()
            .map(|syntax| syntax.filetype.to_string())
            .unwrap_or("no ft".to_string()),
        cfg.cy + 1,
        cfg.numrows
    );
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

// *** Input ***

/// Prompt user to take in input.
///
/// Construct message for prompt using a closure.
/// Take in an optional Callback
fn editor_prompt<F, C>(cfg: &mut EditorConfig, message: F, callback: Option<C>) -> Option<String>
where
    F: Fn(&str) -> String,
    C: Fn(&mut EditorConfig, &str, EditorKey),
{
    let mut buf = String::new();
    loop {
        editor_set_status_msg(cfg, message(&buf));
        editor_refresh_screen(cfg);

        let key = editor_read_key();
        match key {
            EditorKey::EscapeSeq => {
                editor_set_status_msg(cfg, String::new());
                if let Some(cb) = callback.as_ref() {
                    cb(cfg, &buf, key);
                }
                return None;
            }
            EditorKey::CarriageReturn => {
                if buf.len() != 0 {
                    editor_set_status_msg(cfg, String::new());
                    if let Some(cb) = callback.as_ref() {
                        cb(cfg, &buf, key);
                    }
                    return Some(buf);
                }
            }
            EditorKey::Char(ch) => {
                buf.push(ch);
            }
            EditorKey::DeleteKey | EditorKey::Backspace => {
                buf.pop();
            }
            _ => (),
        }
        if let Some(cb) = callback.as_ref() {
            cb(cfg, &buf, key);
        }
    }
}

/// Create Ctrl Codes
///
/// Example: Ctrl-A, Ctrl-B
fn ctrl_key(k: char) -> char {
    (k as u8 & 0x1f) as char
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
        return EditorKey::EscapeSeq;
    } else if c == 127 {
        return EditorKey::Backspace;
    }
    let ch = c as char;
    if ch.is_ascii_control() {
        match ch {
            '\n' | '\r' => EditorKey::CarriageReturn,
            _ => EditorKey::Ctrl(ch),
        }
    } else {
        EditorKey::Char(ch)
    }
}

fn exit_gracefully(cfg: &mut EditorConfig) {
    term_refresh();
    disable_raw_mode(&cfg.term).unwrap();
    exit(0);
}

fn editor_process_keypress(cfg: &mut EditorConfig) {
    let c = editor_read_key();

    match c {
        EditorKey::CarriageReturn => {
            editor_move_cursor(cfg, EditorKey::ArrowRight);
            editor_insert_new_line(cfg);
        }
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
                            "\x1b[31mWARNING!!! File has unsaved changes. \
                            Press Ctrl-Q {} more times to quit.\x1b[39m",
                            cfg.quit_times,
                        ),
                    );
                    cfg.quit_times -= 1;
                    return;
                }
                exit_gracefully(cfg);
            } else if c == ctrl_key('s') {
                editor_save(cfg);
            } else if c == ctrl_key('f') {
                editor_find(cfg);
            }
        }
        EditorKey::Char(c) => {
            editor_insert_char(cfg, c);
        }
        EditorKey::DeleteKey | EditorKey::Backspace => {
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
    editor_select_syntax_highlight(cfg);
    let mut file = File::open(filename).unwrap();
    let mut file_content = Vec::new();
    match file.read_to_end(&mut file_content) {
        Ok(_) => {
            let lossy_content = String::from_utf8_lossy(&file_content);
            for (i, line) in lossy_content.lines().enumerate() {
                editor_insert_row(cfg, line.to_string(), i);
            }
            cfg.dirty = false;
        }
        Err(e) => {
            eprint!("{:?}", e);
            exit_gracefully(cfg);
        }
    }
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
    cfg.filename = editor_prompt(
        cfg,
        |buf| format!("Save as: {} (ESC to Cancel)", buf),
        None::<fn(&mut EditorConfig, &str, EditorKey)>,
    );
    if cfg.filename.is_none() {
        editor_set_status_msg(cfg, "Save aborted!".to_string());
    }
    editor_select_syntax_highlight(cfg);

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
    let mut cfg = EditorConfig::new();
    enable_raw_mode(&cfg).unwrap();

    if args.len() > 1 {
        let filename = &args[1];
        editor_open(&mut cfg, filename);
    }

    editor_set_status_msg(
        &mut cfg,
        "HELP: Ctrl-S = save | Ctrl-Q = quit | Ctrl-F = find".to_string(),
    );

    loop {
        editor_refresh_screen(&mut cfg);
        editor_process_keypress(&mut cfg);
    }
}
