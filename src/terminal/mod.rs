use crate::escape::{Parser, Action};
use crate::config::{self, Config};
use crate::pty::Pty;
use crate::xlib;

use rodio::{Decoder, OutputStream, OutputStreamHandle, source::Source};
use nix::libc;
use arboard::Clipboard;

use std::io::{self, Read, ErrorKind, Write};
use std::time::{Duration, Instant};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::fs::File;
use std::thread;


struct Cell {
    width: i32,
    height: i32,
}

#[derive(Clone, Copy)]
struct Cursor {
    position: Position,
    save: Position,
}

#[derive(Debug)]
pub struct Window {
    pub width: u32,
    pub height: u32,
}

struct Xft {
    font: *mut x11::xft::XftFont,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Position {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy)]
struct Selection {
    start: Position,
    end: Position,
    selecting: bool,
}

struct Sound {
    data: Arc<Vec<u8>>
}

impl AsRef<[u8]> for Sound {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Sound {
    pub fn load(file: &str) -> Result<Sound, Box<dyn std::error::Error>> {
        let mut bell: Vec<u8> = Vec::new();
        File::open(file)?.read_to_end(&mut bell)?;

        Ok(Sound {
            data: Arc::new(bell),
        })
    }

    pub fn decoder(&self) -> Result<Decoder<io::Cursor<Sound>>, Box<dyn std::error::Error>> {
        Ok(Decoder::new(io::Cursor::new(Sound { data: self.data.clone(), }))?)
    }
}

struct Audio {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    bell: Sound,
}

#[derive(Clone, Copy, PartialEq)]
struct Attribute {
    fg: config::UniColor,
    bg: config::UniColor,
}

#[derive(Clone, Copy, PartialEq)]
struct Character {
    attr: Attribute,
    byte: char,
}

impl std::fmt::Debug for Character {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        fmt.write_str(&self.byte.to_string())?;

        Ok(())
    }
}

#[derive(Debug)]
struct ScrollingRegion {
    top: usize,
    bottom: usize,
}

#[derive(Clone, Copy)]
struct Mode {
    decim: bool,
    decom: bool,
    decscnm: bool,
    decckm: bool,
    dectecm: bool,
    decalt: bool,
    decpaste: bool,
    decfocus: bool,
}

#[derive(PartialEq)]
enum CursorStyle {
    Block,
    Line,
    Underline,
}

#[derive(Clone)]
struct AltScreen {
    buf: Vec<Vec<Character>>,
    attr: Attribute,
    mode: Mode,
    cursor: Cursor,
}

impl AltScreen {
    pub fn new(config: &Config, width: usize, height: usize) -> AltScreen {
        let attr = Attribute {
            fg: config.fg,
            bg: config.bg,
        };

        AltScreen {
            cursor: Cursor {
                position: Position {
                    x: 0,
                    y: 0,
                },
                save: Position {
                    x: 0,
                    y: 0,
                },
            },
            attr,
            mode: Mode {
                decim: false,
                decom: false,
                decscnm: false,
                decckm: false,
                dectecm: true,
                decalt: false,
                decpaste: false,
                decfocus: false,
            },
            buf: vec![vec![Character { attr, byte: ' ' }; (width / 10) + 1]; (height / 20) + 1],
        }
    }
}

pub struct Screen {
    display: xlib::Display,
    selection: Selection,
    cursor: Cursor,
    window: Window,
    config: Config,
    audio: Audio,
    attr: Attribute,
    cell: Cell,
    mode: Mode,
    xft: Xft,
    pty: Pty,
    cursor_style: CursorStyle,
    scrolling_region: ScrollingRegion,
    clipboard: Clipboard,
    buf: Vec<Vec<Character>>,
    alt: AltScreen,
    // scrollback: Vec<Vec<Character>>,
    dirty: Vec<Vec<bool>>,
    tabs: Vec<bool>,
    refresh: bool,
    focused: bool,
    scroll_set: bool,
    should_close: bool,
}

pub struct Terminal {
    parser: Parser,
    screen: Screen,
}

impl Screen {
    fn print(&mut self, c: char) {
        // https://www.vt100.net/docs/vt510-rm/IRM.html
        // println!("[print] y={}, x={}, character={:?}", self.cursor.position.y, self.cursor.position.x, c);

        if !self.mode.decim {
            self.set_char(self.cursor.position.y as usize, self.cursor.position.x as usize, Character { attr: self.attr, byte: c });
        } else {
            self.insert_char(self.cursor.position.y as usize, self.cursor.position.x as usize, Character { attr: self.attr, byte: c });
        }

        if self.cursor.position.x < self.window.width as i32 / self.cell.width {
            self.cursor.position.x += 1;
        }
    }

    fn execute(&mut self, byte: u8) {
        // println!("[execute] byte={:#x?}", byte);

        match byte {
            0x09 => {
                self.cursor.position.x += 1;

                while !self.tabs[self.cursor.position.x as usize] {
                    self.cursor.position.x += 1;
                }
            },
            0x0a | 0x0b | 0x0c => {
                if self.cursor.position.y as usize >= self.scrolling_region.bottom {
                    self.scroll_down(self.scrolling_region.bottom);
                } else {
                    self.cursor.position.y += 1;
                }
            },
            0x0d => self.cursor.position.x = 0,
            0x08 => {
                if self.cursor.position.x > 0 {
                    self.cursor.position.x -= 1;
                }
            },
            0x07 => {
                if let Ok(bell) = self.audio.bell.decoder() {
                    if let Err(err) = self.audio.stream_handle.play_raw(bell.convert_samples()) {
                        println!("[+] failed to play bell: {}", err);
                    }
                }
            },
            _ => println!("[+] unknown C0 control code: {:#x?}", byte),
        }
    }

    fn set_char(&mut self, y: usize, x: usize, character: Character) {
        if self.buf[y][x] != character {
            self.buf[y][x] = character;
            self.dirty[y][x] = true;
        }
    }

    fn insert_char(&mut self, y: usize, x: usize, character: Character) {
        self.buf[y].insert(x, character);
        self.buf[y].pop();

        for column in x..self.buf[y].len() {
            self.dirty[y][column] = true;
        }
    }

    fn csi_dispatch(&mut self, params: &[u16], intermediates: &[u8], c: char) -> Result<(), Box<dyn std::error::Error>> {
        /*
        println!(
            "[csi_dispatch] params={:?}, intermediates={:?}, char={:?}, buf_len: {}",
            params, intermediates, c, self.buf.len()
        );
        */

        // let time = Instant::now();

        // thread::sleep(Duration::from_millis(100));

        match c {
            'J' => {
                match params.get(0).unwrap_or(&0) {
                    // default: cursor to end
                    0 => {
                        for line in self.cursor.position.y as usize + 1..self.buf.len() {
                            for column in 0..self.buf[line].len() {
                                self.set_char(line, column, Character { byte: ' ', attr: self.attr });
                            }
                        }

                        for column in self.cursor.position.x as usize..self.buf[self.cursor.position.y as usize].len() {
                            self.set_char(self.cursor.position.y as usize, column, Character { byte: ' ', attr: self.attr });
                        }
                    },
                    // start to cursor
                    1 => {
                        for line in 0..self.cursor.position.y as usize {
                            for column in 0..self.buf[line].len() {
                                self.set_char(line, column, Character { byte: ' ', attr: self.attr });
                            }
                        }

                        for column in 0..self.cursor.position.x as usize + 1 {
                            self.set_char(self.cursor.position.y as usize, column, Character { byte: ' ', attr: self.attr });
                        }
                    },
                    // whole buffer
                    3 | 2 => {
                        for line in 0..self.buf.len() {
                            for column in 0..self.buf[line].len() {
                                self.set_char(line, column, Character { byte: ' ', attr: self.attr });
                            }
                        }
                    },
                    param => println!("[+] expected ED[0..2] found ED{}", param),
                }
            },
            'K' => {
                match params.get(0).unwrap_or(&0) {
                    // default: from cursor to end
                    0 => {
                        for column in self.cursor.position.x as usize..self.buf[self.cursor.position.y as usize].len() {
                            self.set_char(self.cursor.position.y as usize, column, Character { byte: ' ', attr: self.attr });
                        }
                    },
                    // start to cursor
                    1 => {
                        for column in 0..self.cursor.position.x as usize + 1 {
                            self.set_char(self.cursor.position.y as usize, column, Character { byte: ' ', attr: self.attr });
                        }
                    },
                    // whole line
                    2 => {
                        for column in 0..self.buf[self.cursor.position.y as usize].len() {
                            self.set_char(self.cursor.position.y as usize, column, Character { byte: ' ', attr: self.attr });
                        }
                    },
                    param => println!("[+] expected EL[0..2] found EL{}", param),
                }
            },
            'H' | 'f' => {
                self.cursor.position.x = ((*params.get(1).unwrap_or(&1) as i32).max(1) - 1).min(self.window.width as i32 / self.cell.width);

                if self.mode.decom {
                    self.cursor.position.y = (*params.get(0).unwrap_or(&1) as i32).max(1) - 1 + self.scrolling_region.top as i32;
                } else {
                    self.cursor.position.y = (*params.get(0).unwrap_or(&1) as i32).max(1) - 1;
                }
            },
            'A' => {
                self.cursor.position.y -= self.cursor.position.y.min((*params.get(0).unwrap_or(&1) as i32).max(1));
            },
            'B' | 'e' => {
                self.cursor.position.y += (*params.get(0).unwrap_or(&1) as i32).max(1);
            },
            'C' | 'a' => self.cursor.position.x += (*params.get(0).unwrap_or(&1) as i32).max(1),
            'D' => {
                self.cursor.position.x -= self.cursor.position.x.min((*params.get(0).unwrap_or(&1) as i32).max(1));
            },
            'E' => {
                self.cursor.position.y += (*params.get(0).unwrap_or(&1) as i32).max(1);
                self.cursor.position.x = 0;
            },
            'F' => {
                self.cursor.position.y -= self.cursor.position.y.min((*params.get(0).unwrap_or(&1) as i32).max(1));
                self.cursor.position.x = 0;
            },
            'g' => {
                match params.get(0).unwrap_or(&0) {
                    0 => self.tabs[self.cursor.position.x as usize] = false,
                    3 => self.tabs = self.tabs.iter().map(|_| false).collect::<Vec<bool>>(),
                    param => println!("[+] expected TBC[0 | 3] found TBC{}", param),
                }
            },
            '@' => {
                // self.alloc_area(self.cursor.position.x, self.cursor.position.y, 1, *params.get(0).unwrap_or(&1) as i32, false);

                for _ in 0..*params.get(0).unwrap_or(&1) as usize {
                    self.insert_char(self.cursor.position.y as usize, self.cursor.position.x as usize, Character { attr: self.attr, byte: ' ' });
                }
            },
            'i' => {
                // TODO: MC -- copy media
            },
            'G' | '`' => {
                self.cursor.position.x = (*params.get(0).unwrap_or(&1) as i32).max(1) - 1;
            },
            'S' => {
                self.scroll_up(self.scrolling_region.top);
            },
            'T' => {
                self.scroll_down(self.scrolling_region.bottom);
            },
            'L' => {
                /*
                 * this has the same behaviour as kitty, but st seems to keep the x position after
                 * the lines are inserted.
                 *
                 * https://www.vt100.net/docs/vt510-rm/IL.html
                 * "lines scrolled of the page are lost"
                */

                /*
                for index in 0..*params.get(0).unwrap_or(&1) {
                    self.buf.insert((self.cursor.position.y as usize).max(self.scrolling_region.top) + index as usize, vec![Character { attr: self.attr, byte: ' ' }]);
                }

                for index in self.scrolling_region.bottom..self.buf.len() - 1 {
                    self.buf[index] = vec![Character { attr: self.attr, byte: ' ' }];
                }
                */

                for _ in 0..*params.get(0).unwrap_or(&1) as usize {
                    self.scroll_down(self.cursor.position.y as usize);
                }

                self.cursor.position.x = 0;
            },
            'M' => {
                /*
                self.alloc_area(self.cursor.position.x, self.cursor.position.y, *params.get(0).unwrap_or(&1) as i32 + 1, 1, false);

                for _ in y..y + *params.get(0).unwrap_or(&1) as usize {
                    if self.buf.len() > y {
                        self.buf.remove(y);
                    }
                }
                */

                for _ in 0..*params.get(0).unwrap_or(&1) as usize {
                    self.scroll_up(self.cursor.position.y as usize);
                }

                self.cursor.position.x = 0;
            },
            'X' => {
                for index in 0..*params.get(0).unwrap_or(&1) as usize {
                    self.set_char(self.cursor.position.y as usize, self.cursor.position.x as usize + index, Character { byte: ' ', attr: self.attr });
                }
            },
            'P' => {
                for _ in 0..*params.get(0).unwrap_or(&1) as usize {
                    self.buf[self.cursor.position.y as usize].remove(self.cursor.position.x as usize);
                    self.buf[self.cursor.position.y as usize].push(Character { byte: ' ', attr: self.attr });
                }

                for column in self.cursor.position.x as usize..self.buf[self.cursor.position.y as usize].len() {
                    self.dirty[self.cursor.position.y as usize][column] = true;
                }
            },
            'Z' => {
                for _ in 0..*params.get(0).unwrap_or(&1) {
                    self.cursor.position.x -= 1;

                    while !self.tabs[self.cursor.position.x as usize] {
                        self.cursor.position.x -= 1;
                    }
                }

                self.cursor.position.x = self.cursor.position.x.max(0);
            },
            'd' => {
                self.cursor.position.y = (*params.get(0).unwrap_or(&1) as i32).max(1) - 1;
            },
            'm' => {
                let mut index = 0;

                while index < params.len() {
                    let param = params.get(index).unwrap_or(&0);

                    match param {
                        0 => {
                            self.attr = Attribute {
                                fg: self.config.fg,
                                bg: self.config.bg,
                            };
                        },
                        22 => {
                            // set normal intensity
                        },
                        1 => {
                            // set bold, we ignore this for perfomance reasons
                        },
                        3 => {
                            // set italic
                        },
                        7 => {
                            self.attr.fg = self.config.bg;
                            self.attr.bg = self.config.fg;
                        },
                        27 => {
                            self.attr.fg = self.config.fg;
                            self.attr.bg = self.config.bg;
                        },
                        39 => self.attr.fg = self.config.fg,
                        49 => self.attr.bg = self.config.bg,
                        38 | 48 => {
                            match params.get(index + 1).unwrap_or(&2) {
                                2 => {
                                    let raw = xlib::Color::new(
                                        *params.get(index + 2).unwrap_or(&0) as u64,
                                        *params.get(index + 3).unwrap_or(&0) as u64,
                                        *params.get(index + 4).unwrap_or(&0) as u64,
                                    );

                                    if let Ok(xft) = self.display.xft_color_alloc_value(raw) {
                                        if *param == 38 {
                                            self.attr.fg = config::UniColor {
                                                raw,
                                                xft,
                                            };
                                        } else if *param == 48 {
                                            self.attr.bg = config::UniColor {
                                                raw,
                                                xft,
                                            };
                                        }
                                    } else {
                                        println!("[+] failed to create color: {:?}", raw);
                                    }

                                    index += 4;
                                },
                                5 => {},
                                mode => println!("[+] unimplemented SGR mode: {}", mode),
                            }
                        },
                        30..=37 => self.attr.fg = self.config.colors[*param as usize - 30],
                        90..=97 => self.attr.fg = self.config.colors[*param as usize - 90],
                        40..=47 => self.attr.bg = self.config.colors[*param as usize - 40],
                        100..=107 => self.attr.bg = self.config.colors[*param as usize - 100],
                        _ => println!("[+] unknown SGR code: {}", param),
                    }

                    index += 1;
                }
            },
            'n' => {
                match *params.get(0).unwrap_or(&0) {
                    5 => {
                        self.write_tty_raw("\x1b[0n")?;
                    },
                    6 => {
                        if self.mode.decom {
                            self.write_tty_raw(&format!("\x1b[{};{}R", self.cursor.position.y - self.scrolling_region.top as i32 + 1, self.cursor.position.x + 1))?;
                        } else {
                            self.write_tty_raw(&format!("\x1b[{};{}R", self.cursor.position.y + 1, self.cursor.position.x + 1))?;
                        }
                    },
                    param => println!("[+] expected DSR or CPR found {}", param),
                }
            },
            'c' => {
                match *params.get(0).unwrap_or(&0) {
                    14 => self.write_tty_raw("\x1b[>1;4000;33c")?,
                    0 => self.write_tty_raw("\x1b[?6c")?,
                    _ => {},
                }
            },
            's' => self.cursor.save = self.cursor.position,
            'u' => self.cursor.position = self.cursor.save,
            'h' => {
                match *params.get(0).unwrap_or(&0) {
                    1 => self.mode.decckm = true,
                    3 => { /* DECCOLM 80/132 col mode */ },
                    4 => self.mode.decim = true,
                    5 => self.mode.decscnm = true,
                    6 => {
                        // https://git.suckless.org/st/file/st.c.html#l1482
                        self.cursor.position = Position { x: 0, y: 0 };
                        self.mode.decom = true;
                    },
                    7 => { /* auto wrapping */ },
                    12 => { /* start blinking cursor */ },
                    25 => self.mode.dectecm = true,
                    1004 => self.mode.decfocus = true,
                    1049 => {
                        if !self.mode.decalt {
                            self.switch_screen();

                            self.mode.decalt = true;
                        }
                    },
                    2004 => self.mode.decpaste = true,
                    param => println!("[+] unknown mode: {}", param),
                }
            },
            'l' => {
                match *params.get(0).unwrap_or(&0) {
                    1 => self.mode.decckm = false,
                    4 => self.mode.decim = false,
                    5 => self.mode.decscnm = false,
                    6 => {
                        // https://git.suckless.org/st/file/st.c.html#l1482
                        self.cursor.position = Position { x: 0, y: 0 };
                        self.mode.decom = false;
                    },
                    7 => { /* auto wrapping */ },
                    25 => self.mode.dectecm = false,
                    1004 => self.mode.decfocus = false,
                    1049 => {
                        if self.mode.decalt {
                            self.switch_screen();

                            self.mode.decalt = false;
                        }
                    },
                    2004 => self.mode.decpaste = false,
                    param => println!("[+] unknown reset mode: {}", param),
                }
            },
            'q' => {
                match *params.get(0).unwrap_or(&0) {
                    2 => self.cursor_style = CursorStyle::Block,
                    4 => self.cursor_style = CursorStyle::Underline,
                    6 => self.cursor_style = CursorStyle::Line,
                    param => println!("[+] unknown LED: {}", param),
                }
            },
            'r' => {
                self.scrolling_region = ScrollingRegion {
                    top: *params.get(0).unwrap_or(&0).max(&1) as usize - 1,
                    bottom: *params.get(1).unwrap_or(&(self.window.height as u16 / self.cell.height as u16)).max(&1) as usize - 1,
                };

                self.cursor.position = Position {
                    x: 0,
                    y: 0,
                };

                self.scroll_set = !params.is_empty();
            },
            _ => {
                println!(
                    "[csi_dispatch] params={:?}, intermediates={:?}, char={:?}",
                    params, intermediates, c
                );
            },
        }

        if self.mode.decom {
            self.decom_clamp();
        }

        // println!("[csi_dispatch] took {} seconds", time.elapsed().as_secs_f64());

        Ok(())
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], byte: u8) -> Result<(), Box<dyn std::error::Error>> {
        let prefix = intermediates.get(0).unwrap_or(&('q' as u8));
        let unknown: bool;

        /*
        println!(
            "[esc_dispatch] intermediates={:?}, byte={}, buf_len: {}",
            intermediates.iter().map(|x| *x as char).collect::<Vec<char>>(), byte as char, self.buf.len()
        );
        */

        match *prefix as char {
            '(' => {
                match byte as char {
                    'B' => unknown = false, /* ISO 8859-1 */
                    _ => unknown = true,
                }
            },
            'q' | '#' => {
                match byte as char {
                    'M' => {
                        if self.cursor.position.y as usize <= self.scrolling_region.top {
                            self.scroll_up(self.scrolling_region.top);
                        } else {
                            self.cursor.position.y -= 1;
                        }

                        unknown = false;
                    },
                    'D' => {
                        self.cursor.position.y += 1;

                        unknown = false;
                    },
                    'E' => {
                        self.cursor.position.y += 1;
                        self.cursor.position.x = 0;

                        unknown = false;
                    },
                    'Z' => {
                        self.write_tty_raw("\x1b[?6c")?;

                        unknown = false;
                    },
                    'H' => {
                        self.tabs[self.cursor.position.x as usize] = true;

                        unknown = false;
                    },
                    'c' => {
                        let default_ch = Character { attr: Attribute { fg: self.config.fg, bg: self.config.bg }, byte: ' ' };

                        self.buf = vec![vec![default_ch; (self.window.width as usize / self.cell.width as usize) + 1];
                            (self.window.height as usize / self.cell.height as usize) + 1];

                        self.full_dirt();

                        self.cursor.position.x = 0;
                        self.cursor.position.y = 0;

                        self.attr = Attribute {
                            fg: self.config.fg,
                            bg: self.config.bg,
                        };

                        unknown = false;
                    },
                    'B' | '6' => unknown = false,
                    '8' => {
                        self.buf = vec![vec![Character { byte: 'E', attr: self.attr }; (self.window.width as usize / self.cell.width as usize) + 1];
                            (self.window.height as usize / self.cell.height as usize) + 1];

                        unknown = false;
                    },
                    _ => unknown = true,
                }
            },
            _ => unknown = true,
        }

        if unknown {
            println!(
                "[esc_dispatch] intermediates={:?}, byte={}",
                intermediates.iter().map(|x| *x as char).collect::<Vec<char>>(), byte as char
            );
        }

        Ok(())
    }

    fn switch_screen(&mut self) {
        let alt = self.alt.clone();

        self.alt = AltScreen {
            buf: self.buf.clone(),
            cursor: self.cursor,
            attr: self.attr,
            mode: self.mode,
        };

        self.buf = alt.buf;
        self.cursor = alt.cursor;
        self.attr = alt.attr;
        self.mode = alt.mode;

        self.full_dirt();
    }

    #[inline]
    fn full_dirt(&mut self) {
        self.dirty = vec![vec![true; (self.window.width as usize / self.cell.width as usize) + 1]; (self.window.height as usize / self.cell.height as usize) + 1];
    }

    fn scroll_down(&mut self, y: usize) {
        self.buf.remove(self.scrolling_region.top);

        self.buf.insert(y, vec![Character { byte: ' ', attr: self.attr };  (self.window.width as usize / self.cell.width as usize) + 1]);
        self.full_dirt();
    }

    fn scroll_up(&mut self, y: usize) {
        self.buf.remove(self.scrolling_region.bottom);

        self.buf.insert(y, vec![Character { byte: ' ', attr: self.attr }; (self.window.width as usize / self.cell.width as usize) + 1]);
        self.full_dirt();
    }

    fn decom_clamp(&mut self) {
        if self.cursor.position.y < self.scrolling_region.top as i32 {
            self.cursor.position.y = self.scrolling_region.top as i32;
        } else if self.cursor.position.y > self.scrolling_region.bottom as i32 {
            self.cursor.position.y = self.scrolling_region.bottom as i32;
        }
    }

    fn handle_key(&mut self, event: x11::xlib::XKeyEvent) -> Result<(), Box<dyn std::error::Error>> {
        let keysym = self.display.keycode_to_keysym(event.keycode as u8) as u32;

        if is_cursor_key(keysym) {
            let prefix = match self.mode.decckm {
                true => "\x1bO",
                false => "\x1b[",
            };

            let key = match keysym {
                x11::keysym::XK_Up => "A",
                x11::keysym::XK_Down => "B",
                x11::keysym::XK_Left => "D",
                x11::keysym::XK_Right => "C",
                _ => unreachable!(),
            };

            if event.state != 0 {
                // https://git.suckless.org/st/file/config.def.h.html#l327
                self.pty.file.write(format!("\x1b[1;{}{}", event.state + 1, key).as_bytes())?;
            } else {
                self.pty.file.write(format!("{prefix}{key}").as_bytes())?;
            }
        } else if is_special_key(keysym) {
            match keysym {
                x11::keysym::XK_BackSpace => { self.pty.file.write("\x7f".as_bytes())?; },
                x11::keysym::XK_F10 => { self.pty.file.write("\x1b[21~".as_bytes())?; },
                x11::keysym::XK_Escape => { self.pty.file.write("\x1b".as_bytes())?; },
                _ => {},
            }
        } else if keysym == x11::keysym::XK_c && event.state == 5 {
            if let Some(selection) = self.get_selection() {
                self.clipboard.set_text(selection)?;
            }
        } else if keysym == x11::keysym::XK_v && event.state == 5 {
            if let Ok(selection) = self.clipboard.get_text() {
                if self.mode.decpaste {
                    self.write_tty_raw(&format!("\x1b[200~{}\x1b[201~", selection))?;
                } else {
                    self.write_tty_raw(&selection)?;
                }
            }
        } else {
            let mut content = self.display.lookup_string(event)?;

            content = content.chars().filter(|x| *x != '\0').collect();

            if !content.is_empty() {
                self.pty.file.write_all(content.as_bytes())?;
            }
        }

        Ok(())
    }

    fn get_line(&mut self, buf: &Vec<Vec<Character>>, start: Position, end: Position) -> String {
        if buf.len() > start.y as usize {
            let length = buf[start.y as usize].len();

            buf[start.y as usize][(start.x as usize).min(length)..(end.x as usize).min(length)].iter().map(|c| c.byte).collect::<String>()
        } else {
            String::new()
        }
    }

    fn get_selection(&mut self) -> Option<String> {
        let buf = self.buf.clone();

        let mut start = self.selection.start;
        let mut end = self.selection.end;

        if start.y == end.y {
            return if start.x > end.x {
                Some(self.get_line(&buf, end, start))
            } else if start.x < end.x {
                Some(self.get_line(&buf, start, end))
            } else {
                None
            }
        } else {
            if end.y < start.y {
                let old_start = start;

                start = end;
                end = old_start;
            }

            let mut content = String::new();

            for y in start.y..=end.y {
                if y == start.y && self.buf.len() as i32 > y {
                    'start: for x in start.x as usize..self.window.width as usize / self.cell.width as usize {
                        if x < self.buf[start.y as usize].len() {
                            content.push(self.buf[start.y as usize][x].byte);
                        } else {
                            break 'start;
                        }
                    }
                } else if y == end.y && self.buf.len() as i32 > y {
                    'end: for x in 0..end.x as usize {
                        if x < self.buf[end.y as usize].len() {
                            content.push(self.buf[end.y as usize][x].byte);
                        } else {
                            break 'end;
                        }
                    }
                } else if self.buf.len() as i32 > y {
                    content.extend(self.buf[y as usize].iter().map(|c| c.byte).collect::<Vec<char>>());
                }

                content.push('\n');
            }

            Some(content)
        }
    }

    fn write_tty_raw(&mut self, content: &str) -> Result<(), Box<dyn std::error::Error>> {
        if !content.is_empty() {
            self.pty.file.write_all(content.as_bytes())?;
        }

        Ok(())
    }

    fn handle_event(&mut self, event: x11::xlib::XEvent) -> Result<(), Box<dyn std::error::Error>> {
        match unsafe { event.type_ } {
            x11::xlib::KeyPress => {
                self.handle_key(unsafe { event.key })?;
            },
            x11::xlib::ButtonPress => {
                match unsafe { event.button.button } {
                    x11::xlib::Button4 => {
                        self.write_tty_raw("\x19")?;

                        self.refresh = true;
                    },
                    x11::xlib::Button5 => {
                        self.write_tty_raw("\x05")?;

                        self.refresh = true;
                    },
                    x11::xlib::Button1 => {
                        let raw = unsafe { event.button.y };
                        let y = raw.is_negative().then(|| raw - self.cell.height).unwrap_or(raw) / self.cell.height;

                        self.selection.start = Position {
                            x: unsafe { event.button.x } / self.cell.width,
                            y,
                        };

                        self.selection.end = Position {
                            x: unsafe { event.button.x } / self.cell.width,
                            y,
                        };

                        self.selection.selecting = true;
                        self.refresh = true;
                    },
                    _ => {},
                }
            },
            x11::xlib::ButtonRelease => {
                match unsafe { event.button.button } {
                    x11::xlib::Button1 => {
                        self.selection.selecting = false;
                    },
                    _ => {},
                }
            },
            x11::xlib::MotionNotify => {
                if self.selection.selecting {
                    let raw = unsafe { event.motion.y };
                    let y = raw.is_negative().then(|| raw - self.cell.height).unwrap_or(raw) / self.cell.height;

                    self.selection.end = Position {
                        x: unsafe { event.motion.x } / self.cell.width,
                        y,
                    };

                    self.refresh = true;
                }
            },
            x11::xlib::Expose => {
                /*
                 * TODO: screenshots seem to cause a Expose event with a fucked up width and height
                 *
                 * we may need to handle other forms of resizing
                 *
                */

                let width = unsafe { event.expose.width } as u32;
                let height = unsafe { event.expose.height } as u32;

                if width != self.window.width || height != self.window.height {
                    self.window = Window {
                        width,
                        height,
                    };

                    self.display.resize_back_buffer(&self.window);
                    self.pty.resize(width as u16 / self.cell.width as u16, height as u16 / self.cell.height as u16)?;
                    self.dirty = vec![vec![true; (width as usize / self.cell.width as usize) + 1]; (height as usize / self.cell.height as usize) + 1];

                    let default_ch = Character { attr: Attribute { fg: self.config.fg, bg: self.config.bg }, byte: ' ' };

                    self.buf.resize((height as usize / self.cell.height as usize) + 1, vec![default_ch; (width as usize / self.cell.width as usize) + 1]);
                    self.alt.buf.resize((height as usize / self.cell.height as usize) + 1, vec![default_ch; (width as usize / self.cell.width as usize) + 1]);

                    self.buf.iter_mut().for_each(|line| line.resize((width as usize / self.cell.width as usize) + 1, default_ch));
                    self.alt.buf.iter_mut().for_each(|line| line.resize((width as usize / self.cell.width as usize) + 1, default_ch));

                    if !self.scroll_set {
                        self.scrolling_region.bottom = (self.window.height as usize / self.cell.height as usize) - 1;
                    }

                    if self.cursor.position.y > self.window.height as i32 / self.cell.height {
                        self.cursor.position.y = self.window.height as i32 / self.cell.height - 1;
                    }

                    self.refresh = true;
                }
            },
            x11::xlib::VisibilityNotify => {
                self.dirty = vec![vec![true; (self.window.width as usize / self.cell.width as usize) + 1]; (self.window.height as usize / self.cell.height as usize) + 1];

                self.refresh = true
            },
            x11::xlib::FocusIn => {
                if self.mode.decfocus {
                    self.write_tty_raw("\x1b[I")?;
                }

                self.focused = true;
                self.refresh = true;
            },
            x11::xlib::FocusOut => {
                if self.mode.decfocus {
                    self.write_tty_raw("\x1b[O")?;
                }

                self.focused = false;
                self.refresh = true;
            },
            _ => {},
        }

        Ok(())
    }

    #[inline]
    fn is_within_selection(&self, y: usize, x: usize, selection: &Selection) -> bool {
        if selection.start == selection.end {
            false
        } else if selection.start.y == selection.end.y && y as i32 == selection.start.y {
            x as i32 >= selection.start.x && (x as i32) < selection.end.x
        } else if y as i32 == selection.start.y {
            x as i32 >= selection.start.x
        } else if y as i32 == selection.end.y {
            x as i32 <= selection.end.x
        } else if y as i32 > selection.start.y && (y as i32) < selection.end.y {
            true
        } else {
            false
        }
    }

    fn draw(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        /* making sure end.y is always bigger then start.y and end.x is always bigger start.x */

        let mut selection = if self.selection.end.y > self.selection.start.y {
            self.selection
        } else {
            Selection {
                start: self.selection.end,
                end: self.selection.start,
                selecting: false,
            }
        };

        if selection.start.x > selection.end.x && selection.start.y == selection.end.y {
            let end = selection.end.x;

            selection.end.x = selection.start.x;
            selection.start.x = end;
        }

        for (y, line) in self.buf.iter().enumerate().rev() {
            let y_pos = y as i32 * self.cell.height;

            if (0..self.window.height as i32).contains(&y_pos) {
                for (x, character) in line.iter().enumerate() {
                    let is_within_selection = self.is_within_selection(y, x, &selection);

                    if self.dirty[y][x] || is_within_selection {
                        if is_within_selection {
                            self.dirty[y][x] = true;
                        } else {
                            self.dirty[y][x] = false;
                        }

                        self.display.draw_rec(
                            x as i32 * self.cell.width,
                            y_pos,
                            self.cell.width as u32,
                            self.cell.height as u32,
                            if is_within_selection {
                                character.attr.fg.raw
                            } else {
                                character.attr.bg.raw
                            }
                        );

                        self.display.xft_draw_string(
                            character.byte.to_string().as_str(),
                            x as i32 * self.cell.width,
                            y_pos + 15,
                            self.xft.font,
                            if is_within_selection {
                                &character.attr.bg.xft
                            } else {
                                &character.attr.fg.xft
                            }
                        );
                    }
                }
            }
        }

        if self.mode.dectecm {
            let width = match self.cursor_style {
                CursorStyle::Block | CursorStyle::Underline => self.cell.width as u32,
                CursorStyle::Line => 2,
            };

            let height = match self.cursor_style {
                CursorStyle::Block | CursorStyle::Line => self.cell.height as u32,
                CursorStyle::Underline => 5,
            };

            let y = match self.cursor_style {
                CursorStyle::Block | CursorStyle::Line => self.cursor.position.y * self.cell.height,
                CursorStyle::Underline => (self.cursor.position.y * self.cell.height) + 15,
            };

            if !self.focused && self.cursor_style == CursorStyle::Block {
                self.display.outline_rec(
                    self.cursor.position.x * self.cell.width,
                    self.cursor.position.y * self.cell.height,
                    self.cell.width as u32 - 1,
                    self.cell.height as u32 - 1,
                    self.config.fg.raw,
                );
            } else {
                self.display.draw_rec(
                    self.cursor.position.x * self.cell.width,
                    y,
                    width,
                    height,
                    self.config.fg.raw,
                );
            }
        }

        self.dirty[self.cursor.position.y as usize][self.cursor.position.x as usize] = true;

        self.display.swap_buffers(&self.window);

        self.refresh = false;

        Ok(())
    }
}

impl Terminal {
    pub fn new() -> Result<Terminal, Box<dyn std::error::Error>> {
        let mut display = xlib::Display::open()?;

        let window_attr = display.get_window_attributes();

        let (_stream, stream_handle) = OutputStream::try_default()?;

        let config = Config::load(&display)?;

        let font = display.load_font(&config.font)?;

        let attr = Attribute {
            fg: config.fg,
            bg: config.bg,
        };

        let alt = AltScreen::new(&config, window_attr.width as usize, window_attr.height as usize);

        let tabs = (0..config.tab_max).map(|x| x % 8 == 0).collect::<Vec<bool>>();

        let bell = Sound::load(&config.bell)?;

        Ok(Terminal {
            parser: Parser::new(),
            screen: Screen {
                display,
                selection: Selection {
                    start: Position { x: 0, y: 0 },
                    end: Position { x: 0, y: 0 },
                    selecting: false,
                },
                cursor: Cursor {
                    position: Position {
                        x: 0,
                        y: 0,
                    },
                    save: Position {
                        x: 0,
                        y: 0,
                    },
                },
                window: Window {
                    width: window_attr.width as u32,
                    height: window_attr.height as u32,
                },
                attr,
                config,
                audio: Audio {
                    _stream,
                    stream_handle,
                    bell,
                },
                cell: Cell {
                    width: 10,
                    height: 20,
                },
                mode: Mode {
                    decim: false,
                    decom: false,
                    decscnm: false,
                    decckm: false,
                    dectecm: true,
                    decalt: false,
                    decpaste: false,
                    decfocus: false,
                },
                xft: Xft {
                    font,
                },
                cursor_style: CursorStyle::Block,
                scrolling_region: ScrollingRegion {
                    top: 0,
                    bottom: (window_attr.height as usize / 20 as usize) - 1,
                },
                clipboard: Clipboard::new()?,
                pty: Pty::new()?,
                buf: vec![vec![Character { attr, byte: ' ' }; (window_attr.width as usize / 10) + 1]; (window_attr.height as usize / 20) + 1],
                alt,
                tabs,
                dirty: vec![vec![true; (window_attr.width as usize / 10) + 1]; (window_attr.height as usize / 20) + 1],
                refresh: true,
                focused: true,
                scroll_set: false,
                should_close: false,
            },
        })
    }

    fn read_tty(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut more_to_read = true;

        while more_to_read {
            let mut buffer: Vec<u8> = vec![0; 2048];

            match self.screen.pty.file.read(&mut buffer) {
                Ok(0) => {},
                Ok(bytes) => {
                    self.handle_bytes(&buffer[..bytes])?;
                },
                Err(err) => {
                    match err.kind() {
                        ErrorKind::WouldBlock => more_to_read = false,
                        ErrorKind::Interrupted => {},
                        _ => return Err(Box::new(err)),
                    }
                },
            }
        }

        Ok(())
    }

    fn handle_bytes(&mut self, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        for byte in bytes {
            if let Ok(Some(action)) = self.parser.advance(*byte) {
                match action {
                    Action::Print(c) => {
                        self.screen.print(c);
                    },
                    Action::Execute(byte) => {
                        self.screen.execute(byte);
                    },
                    Action::CsiDispatch(params, intermediates, c) => {
                        self.screen.csi_dispatch(&params, intermediates, c)?;
                    },
                    Action::EscDispatch(intermediates, c) => {
                        self.screen.esc_dispatch(intermediates, c)?;
                    },
                    Action::OscDispatch(_) => {},
                }
            }
        }

        self.screen.refresh = true;

        Ok(())
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.screen.display.set_window_name("termal");
        self.screen.display.define_cursor();
        self.screen.display.select_input();
        self.screen.display.map_window();
        self.screen.display.flush();

        unsafe {
            let flags = libc::fcntl(self.screen.pty.file.as_raw_fd(), libc::F_GETFL, 0) | libc::O_NONBLOCK;

            libc::fcntl(self.screen.pty.file.as_raw_fd(), libc::F_SETFL, flags);
        }

        while !self.screen.should_close {
            let render_time = Instant::now();

            self.read_tty()?;

            if let Some(events) = self.screen.display.poll_event() {
                for event in events {
                    self.screen.handle_event(event)?;
                }
            }

            if self.screen.refresh {
                self.screen.draw()?;
            }

            thread::sleep(Duration::from_millis(8 - render_time.elapsed().subsec_millis().min(8) as u64));
        }

        Ok(())
    }
}

fn is_cursor_key(keysym: u32) -> bool {
    matches!(
        keysym,
        x11::keysym::XK_Up
        | x11::keysym::XK_Down
        | x11::keysym::XK_Left
        | x11::keysym::XK_Right
    )
}

fn is_special_key(keysym: u32) -> bool {
    matches!(
        keysym,
        x11::keysym::XK_Up
        | x11::keysym::XK_BackSpace
        | x11::keysym::XK_F10
        | x11::keysym::XK_Escape
    )
}


