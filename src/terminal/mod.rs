use crate::escape::{Parser, Action};
use crate::config::{self, Config};
use crate::pty::Pty;
use crate::xlib;

use rodio::{Decoder, OutputStream, OutputStreamHandle, source::Source};
use nix::libc;

use std::io::{self, Read, ErrorKind, Write};
use std::time::{Duration, Instant};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::fs::File;
use std::thread;

const DEFAULT_TAB_MAX: usize = 400;


struct Cell {
    width: i32,
    height: i32,
}

struct Cursor {
    position: Position,
    save: Position,
    scroll: i32,
}

#[derive(Debug)]
pub struct Window {
    pub width: u32,
    pub height: u32,
}

struct Xft {
    font: *mut x11::xft::XftFont,
}

#[derive(Clone, Copy)]
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

#[derive(Clone, Copy)]
struct Attribute {
    fg: config::UniColor,
    bg: config::UniColor,
}

#[derive(Clone, Copy)]
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

struct ScrollingRegion {
    top: usize,
    bottom: usize,
}

struct Mode {
    decim: bool,
    decom: bool,
    decscnm: bool,
}

#[derive(PartialEq)]
enum CursorStyle {
    Block,
    Line,
    Underline,
}

pub struct Terminal {
    display: xlib::Display,
    selection: Selection,
    parser: Parser,
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
    buf: Vec<Vec<Character>>,
    tabs: Vec<bool>,
    refresh: bool,
    focused: bool,
}

impl Terminal {
    fn print(&mut self, c: char) {
        self.alloc_area(self.cursor.position.x, self.cursor.position.y, 1, 1);

        // https://www.vt100.net/docs/vt510-rm/IRM.html
        if !self.mode.decim {
            self.buf[self.cursor.position.y as usize][self.cursor.position.x as usize] = Character { attr: self.attr, byte: c };
        } else {
            /*
             * st seems to remove characters that go outside the buffer but kitty doesnt,
             * currently this is commented out as we keep things consistent by doing things the
             * same way kitty does.
             *
            */

            //let right = self.buf[self.cursor.position.y as usize].len();

            self.buf[self.cursor.position.y as usize].insert(self.cursor.position.x as usize, Character { attr: self.attr, byte: c });

            /*
            if right < self.buf[self.cursor.position.y as usize].len() {
                self.buf[self.cursor.position.y as usize].drain(right..);
            }
            */
        }

        self.cursor.position.x += 1;
    }

    fn execute(&mut self, byte: u8) {
        println!("[execute] byte={:#x?}", byte);

        match byte {
            0x09 => {
                self.cursor.position.x += 1;

                while !self.tabs[self.cursor.position.x as usize] {
                    self.cursor.position.x += 1;
                }
            },
            0x0a | 0x0b | 0x0c => {
                if self.cursor.position.y as usize >= self.scrolling_region.bottom {
                    self.buf.remove(self.scrolling_region.top);

                    self.buf.insert(self.scrolling_region.bottom, vec![Character { byte: ' ', attr: self.attr }]);
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

    fn csi_dispatch(&mut self, params: &[u16], intermediates: &[u8], c: char) -> Result<(), Box<dyn std::error::Error>> {
        println!(
            "[csi_dispatch] params={:?}, intermediates={:?}, char={:?}, x={}, y={}",
            params, intermediates, c, self.cursor.position.x, self.cursor.position.y
        );

        // thread::sleep(Duration::from_millis(100));

        match c {
            'J' => {
                self.alloc_area(self.cursor.position.x, self.cursor.position.y, 1, 1);

                match params.get(0).unwrap_or(&0) {
                    // default
                    0 => {
                        self.buf.drain(self.cursor.position.y as usize + 1..);

                        for index in self.cursor.position.x as usize..self.buf[self.cursor.position.y as usize].len() {
                            self.buf[self.cursor.position.y as usize][index] = Character { byte: ' ', attr: self.attr };
                        }
                    },
                    // start to cursor
                    1 => {
                        for index in 0..self.cursor.position.y as usize {
                            self.buf[index] = vec![Character { byte: ' ', attr: self.attr }];
                        }

                        for index in 0..self.cursor.position.x as usize + 1 {
                            self.buf[self.cursor.position.y as usize][index] = Character { byte: ' ', attr: self.attr };
                        }
                    },
                    // whole buffer
                    3 | 2 => { self.buf.drain(..); },
                    param => println!("[+] expected ED[0..2] found ED{}", param),
                }
            },
            'K' => {
                self.alloc_area(self.cursor.position.x, self.cursor.position.y, 1, 1);

                match params.get(0).unwrap_or(&0) {
                    // default: from cursor to end
                    0 => { self.buf[self.cursor.position.y as usize].drain(self.cursor.position.x as usize..); },
                    // start to cursor
                    1 => {
                        for index in 0..self.cursor.position.x as usize + 1 {
                            self.buf[self.cursor.position.y as usize][index] = Character { byte: ' ', attr: self.attr };
                        }
                    },
                    // whole line
                    2 => { self.buf[self.cursor.position.y as usize].drain(..); },
                    param => println!("[+] expected EL[0..2] found EL{}", param),
                }
            },
            'H' | 'f' => {
                self.cursor.position.x = (*params.get(1).unwrap_or(&1) as i32).max(1) - 1;

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
                self.alloc_area(self.cursor.position.x, self.cursor.position.y, 1, *params.get(0).unwrap_or(&1) as i32);

                for _ in 0..*params.get(0).unwrap_or(&1) as usize {
                    self.buf[self.cursor.position.y as usize].insert(self.cursor.position.x as usize, Character { attr: self.attr, byte: ' ' });
                }
            },
            'i' => {
                // TODO: MC -- copy media
            },
            'G' | '`' => {
                self.cursor.position.x = (*params.get(0).unwrap_or(&1) as i32).max(1) - 1;
            },
            'S' => {
                /*
                 * these are implemented wrong as we need to use the scroll buffer to scroll
                 * just like in L or M
                */
                self.cursor.scroll += *params.get(0).unwrap_or(&1) as i32;
            },
            'T' => {
                self.cursor.scroll -= *params.get(0).unwrap_or(&1) as i32;
            },
            'L' => {
                /*
                 * this has the same behaviour as kitty, but st seems to keep the x position after
                 * the lines are inserted.
                 *
                 * https://www.vt100.net/docs/vt510-rm/IL.html
                 * "lines scrolled of the page are lost"
                */

                for index in 0..*params.get(0).unwrap_or(&1) {
                    self.buf.insert((self.cursor.position.y as usize).max(self.scrolling_region.top) + index as usize, vec![Character { attr: self.attr, byte: ' ' }]);
                }

                for index in self.scrolling_region.bottom..self.buf.len() - 1 {
                    self.buf[index] = vec![Character { attr: self.attr, byte: ' ' }];
                }

                self.cursor.position.x = 0;
            },
            'M' => {
                self.alloc_area(self.cursor.position.x, self.cursor.position.y, *params.get(0).unwrap_or(&1) as i32 + 1, 1);

                let y = (self.cursor.position.y as usize).max(self.scrolling_region.top);

                for _ in y..y + *params.get(0).unwrap_or(&1) as usize {
                    if self.buf.len() > y {
                        self.buf.remove(y);
                    }
                }

                self.cursor.position.x = 0;
            },
            'X' => {
                self.alloc_area(self.cursor.position.x, self.cursor.position.y, 1, *params.get(0).unwrap_or(&1) as i32);

                for index in 0..*params.get(0).unwrap_or(&1) as usize {
                    self.buf[self.cursor.position.y as usize][self.cursor.position.x as usize + index] = Character { byte: ' ', attr: self.attr };
                }
            },
            'P' => {
                self.alloc_area(self.cursor.position.x, self.cursor.position.y, 1, *params.get(0).unwrap_or(&1) as i32);

                self.buf[self.cursor.position.y as usize].drain(self.cursor.position.x as usize..self.cursor.position.x as usize + *params.get(0).unwrap_or(&1) as usize);
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
                for param in params {
                    match param {
                        0 => {
                            self.attr = Attribute {
                                fg: self.config.fg,
                                bg: self.config.bg,
                            };
                        },
                        39 => {
                            self.attr.fg = self.config.fg;
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
                            let fg = self.attr.fg;

                            self.attr.fg = self.attr.bg;
                            self.attr.bg = fg;
                        },
                        38 | 48 => {
                            match params.get(1).unwrap_or(&2) {
                                2 => {
                                    let raw = xlib::Color::new(
                                        *params.get(2).unwrap_or(&0) as u64,
                                        *params.get(3).unwrap_or(&0) as u64,
                                        *params.get(4).unwrap_or(&0) as u64,
                                    );

                                    if let Ok(xft) = self.display.xft_color_alloc_name(raw) {
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
                    1 => { /* DECCKM cursor key */ },
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
                    25 => { /* make cursor visible */ },
                    1049 => { /* swap screen */ },
                    2004 => { /* i dont even know what this code is lol */ },
                    param => println!("[+] unknown mode: {}", param),
                }
            },
            'l' => {
                match *params.get(0).unwrap_or(&0) {
                    4 => self.mode.decim = false,
                    5 => self.mode.decscnm = false,
                    6 => {
                        // https://git.suckless.org/st/file/st.c.html#l1482
                        self.cursor.position = Position { x: 0, y: 0 };
                        self.mode.decom = false;
                    },
                    _ => self.mode = Mode { decim: false, decom: false, decscnm: false },
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

        Ok(())
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], byte: u8) -> Result<(), Box<dyn std::error::Error>> {
        let prefix = intermediates.get(0).unwrap_or(&('q' as u8));
        let unknown: bool;

        println!(
            "[esc_dispatch] intermediates={:?}, byte={}",
            intermediates.iter().map(|x| *x as char).collect::<Vec<char>>(), byte as char
        );

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
                            self.buf.remove(self.scrolling_region.bottom);

                            self.buf.insert(self.scrolling_region.top, vec![Character { byte: ' ', attr: self.attr }]);
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
                    'B' | '6' => unknown = false,
                    '8' => {
                        self.buf = vec![vec![Character { byte: 'E', attr: self.attr }; self.window.width as usize / self.cell.width as usize];
                            self.window.height as usize / self.cell.height as usize];

                        unknown = false
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
}

impl Terminal {
    pub fn new() -> Result<Terminal, Box<dyn std::error::Error>> {
        let mut display = xlib::Display::open()?;

        let font = display.load_font("DejaVu Sans Mono:size=11:antialias=true")?;
        let window_attr = display.get_window_attributes();

        let (_stream, stream_handle) = OutputStream::try_default()?;

        let bell = Sound::load("assets/pluh.wav")?;

        let config = Config::load(&display)?;

        let attr = Attribute {
            fg: config.fg,
            bg: config.bg,
        };

        Ok(Terminal {
            display,
            selection: Selection {
                start: Position { x: 0, y: 0 },
                end: Position { x: 0, y: 0 },
                selecting: false,
            },
            parser: Parser::new(),
            cursor: Cursor {
                position: Position {
                    x: 0,
                    y: 0,
                },
                save: Position {
                    x: 0,
                    y: 0,
                },
                scroll: 0,
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
            },
            xft: Xft {
                font,
            },
            cursor_style: CursorStyle::Block,
            scrolling_region: ScrollingRegion {
                top: 0,
                bottom: window_attr.height as usize / 20,
            },
            pty: Pty::new()?,
            buf: Vec::new(),
            tabs: (0..DEFAULT_TAB_MAX).map(|x| x % 8 == 0).collect::<Vec<bool>>(),
            refresh: true,
            focused: true,
        })
    }

    fn decom_clamp(&mut self) {
        if self.cursor.position.y < self.scrolling_region.top as i32 {
            self.cursor.position.y = self.scrolling_region.top as i32;
        } else if self.cursor.position.y > self.scrolling_region.bottom as i32 {
            self.cursor.position.y = self.scrolling_region.bottom as i32;
        }
    }

    fn write_tty(&mut self, event: x11::xlib::XKeyEvent) -> Result<(), Box<dyn std::error::Error>> {
        match self.display.keycode_to_keysym(event.keycode as u8) as u32 {
            x11::keysym::XK_Up => { self.pty.file.write("\x1b[A".as_bytes())?; },
            x11::keysym::XK_Down => { self.pty.file.write("\x1b[B".as_bytes())?; },
            x11::keysym::XK_Left => { self.pty.file.write("\x1b[D".as_bytes())?; },
            x11::keysym::XK_Right => { self.pty.file.write("\x1b[C".as_bytes())?; },
            x11::keysym::XK_BackSpace => { self.pty.file.write("\x7f".as_bytes())?; },
            x11::keysym::XK_F10 => { self.pty.file.write("\x1b[21~".as_bytes())?; },
            x11::keysym::XK_Escape => { self.pty.file.write("\x1b".as_bytes())?; },
            _ => {
                let mut content = self.display.lookup_string(event)?;

                content = content.chars().filter(|x| *x != '\0').collect();

                if !content.is_empty() {
                    self.pty.file.write_all(content.as_bytes())?;
                }
            },
        }

        Ok(())
    }

    fn write_tty_raw(&mut self, content: &str) -> Result<(), Box<dyn std::error::Error>> {
        if !content.is_empty() {
            self.pty.file.write_all(content.as_bytes())?;
        }

        Ok(())
    }

    fn read_tty(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut buffer: Vec<u8> = vec![0; 2048];
        // let mut buffer: Vec<u8> = vec![0; 1];

        match self.pty.file.read(&mut buffer) {
            Ok(0) => {},
            Ok(bytes) => {
                self.handle_bytes(&buffer[..bytes])?;
            },
            Err(err) => {
                match err.kind() {
                    ErrorKind::WouldBlock => {},
                    ErrorKind::Interrupted => {},
                    _ => return Err(Box::new(err)),
                }
            },
        }

        Ok(())
    }

    fn alloc_area(&mut self, x: i32, y: i32, height: i32, width: i32) {
        if y as usize >= self.buf.len() {
            for _ in self.buf.len()..y as usize + height as usize {
                self.buf.push(Vec::new());
            }
        }

        if x as usize >= self.buf[y as usize].len() {
            for _ in self.buf[y as usize].len()..x as usize + width as usize {
                self.buf[y as usize].push(Character { attr: self.attr, byte: ' ' });
            }
        }
    }

    fn handle_bytes(&mut self, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        for byte in bytes {
            // println!("byte: {:?}", *byte as char);

            if let Ok(Some(action)) = self.parser.advance(*byte) {
                // println!("action: {:?}", action);

                match action {
                    Action::Print(c) => {
                        self.print(c);
                    },
                    Action::Execute(byte) => {
                        self.execute(byte);
                    },
                    Action::CsiDispatch(params, intermediates, c) => {
                        self.csi_dispatch(&params, &intermediates, c)?;
                    },
                    Action::EscDispatch(intermediates, c) => {
                        self.esc_dispatch(&intermediates, c)?;
                    },
                    Action::OscDispatch(_) => {},
                }
            }
        }

        self.refresh = true;

        Ok(())
    }

    fn handle_event(&mut self, event: x11::xlib::XEvent) -> Result<(), Box<dyn std::error::Error>> {
        match unsafe { event.type_ } {
            x11::xlib::KeyPress => {
                self.write_tty(unsafe { event.key })?;
            },
            x11::xlib::ButtonPress => {
                match unsafe { event.button.button } {
                    x11::xlib::Button4 => {
                        self.cursor.scroll += self.cell.height;

                        self.refresh = true;
                    },
                    x11::xlib::Button5 => {
                        self.cursor.scroll -= self.cell.height;

                        self.refresh = true;
                    },
                    x11::xlib::Button1 => {
                        self.selection.start = Position {
                            x: unsafe { event.button.x } / self.cell.width,
                            y: unsafe { event.button.y - self.cursor.scroll } / self.cell.height,
                        };

                        self.selection.end = Position {
                            x: unsafe { event.button.x } / self.cell.width,
                            y: unsafe { event.button.y - self.cursor.scroll } / self.cell.height,
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
                    self.selection.end = Position {
                        x: unsafe { event.motion.x } / self.cell.width,
                        y: unsafe { event.motion.y - self.cursor.scroll } / self.cell.height,
                    };

                    self.refresh = true;
                }
            },
            x11::xlib::Expose => {
                let width = unsafe { event.expose.width } as u32;
                let height = unsafe { event.expose.height } as u32;

                self.window = Window {
                    width,
                    height,
                };

                self.display.resize_back_buffer(&self.window);
                self.pty.resize(width as u16 / self.cell.width as u16, height as u16 / self.cell.height as u16)?;
            },
            x11::xlib::VisibilityNotify => self.refresh = true,
            x11::xlib::FocusIn => {
                self.focused = true;
                self.refresh = true;
            },
            x11::xlib::FocusOut => {
                self.focused = false;
                self.refresh = true;
            },
            _ => {},
        }

        Ok(())
    }

    fn draw(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.display.draw_rec(0, 0, self.window.width, self.window.height, self.config.bg.raw);

        for (y, line) in self.buf.iter().enumerate() {
            let y_pos = (y as i32 * self.cell.height) + self.cursor.scroll;

            if (0..self.window.height as i32).contains(&y_pos) {
                for (x, character) in line.iter().enumerate() {
                    if character.byte != '\0' {
                        self.display.draw_rec(
                            x as i32 * self.cell.width,
                            y_pos,
                            self.cell.width as u32,
                            self.cell.height as u32,
                            character.attr.bg.raw
                        );

                        self.display.xft_draw_string(
                            character.byte.to_string().as_str(),
                            x as i32 * self.cell.width,
                            (y as i32 * self.cell.height) + 15 + self.cursor.scroll,
                            self.xft.font,
                            &character.attr.fg.xft,
                        );
                    }
                }
            }
        }

        if self.selection.start.y == self.selection.end.y {
            if self.selection.start.x != self.selection.end.x {
                self.display.outline_rec(
                    self.selection.start.x.min(self.selection.end.x) * self.cell.width,
                    (self.selection.start.y * self.cell.height) + self.cursor.scroll,
                    (self.selection.end.x.max(self.selection.start.x) - self.selection.start.x.min(self.selection.end.x)) as u32 * self.cell.width as u32,
                    self.cell.height as u32,
                    self.config.fg.raw,
                );
            }
        } else {
            let selection = if self.selection.end.y > self.selection.start.y {
                self.selection
            } else {
                Selection {
                    start: self.selection.end,
                    end: self.selection.start,
                    selecting: false,
                }
            };

            for y in selection.start.y..selection.end.y + 1 {
                if y == selection.start.y {
                    self.display.outline_rec(
                        selection.start.x * self.cell.width,
                        (y * self.cell.height) + self.cursor.scroll,
                        ((self.window.width as i32 / self.cell.width) - selection.start.x) as u32 * self.cell.width as u32,
                        self.cell.height as u32,
                        self.config.fg.raw,
                    );
                } else if y == selection.end.y {
                    self.display.outline_rec(
                        0,
                        (y * self.cell.height) + self.cursor.scroll,
                        selection.end.x as u32 * self.cell.width as u32,
                        self.cell.height as u32,
                        self.config.fg.raw,
                    );
                } else {
                    self.display.outline_rec(
                        0,
                        (y * self.cell.height) + self.cursor.scroll,
                        (self.window.width as i32 / self.cell.width) as u32 * self.cell.width as u32,
                        self.cell.height as u32,
                        self.config.fg.raw,
                    );
                }
            }
        }

        let width = match self.cursor_style {
            CursorStyle::Block | CursorStyle::Underline => self.cell.width as u32,
            CursorStyle::Line => 2,
        };

        let height = match self.cursor_style {
            CursorStyle::Block | CursorStyle::Line => self.cell.height as u32,
            CursorStyle::Underline => 5,
        };

        let y = match self.cursor_style {
            CursorStyle::Block | CursorStyle::Line => (self.cursor.position.y * self.cell.height) + self.cursor.scroll,
            CursorStyle::Underline => (self.cursor.position.y * self.cell.height) + self.cursor.scroll + 15,
        };

        if !self.focused && self.cursor_style == CursorStyle::Block {
            self.display.outline_rec(
                self.cursor.position.x * self.cell.width,
                (self.cursor.position.y * self.cell.height) + self.cursor.scroll,
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

        self.display.swap_buffers(&self.window);

        self.refresh = false;

        Ok(())
    }

    fn auto_scroll(&mut self) {
        let scroll = -(self.cursor.scroll);

        if self.cursor.position.y * self.cell.height > scroll + self.window.height as i32 - self.cell.height {
            self.cursor.scroll = -((self.cursor.position.y * self.cell.height) - self.window.height as i32 + self.cell.height);

            self.refresh = true;
        } else if self.cursor.position.y * self.cell.height < scroll {
            self.cursor.scroll = -(self.cursor.position.y * self.cell.height);

            self.refresh = true;
        }
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.display.set_window_name("termal");
        self.display.define_cursor();
        self.display.select_input();
        self.display.map_window();
        self.display.flush();

        /*
         * [ ] get htop and nvim to run properly
         *
        */

        unsafe {
            let flags = libc::fcntl(self.pty.file.as_raw_fd(), libc::F_GETFL, 0) | libc::O_NONBLOCK;

            libc::fcntl(self.pty.file.as_raw_fd(), libc::F_SETFL, flags);
        }

        loop {
            let render_time = Instant::now();

            self.auto_scroll();
            self.read_tty()?;

            if let Some(events) = self.display.poll_event() {
                for event in events {
                    self.handle_event(event)?;
                }
            }

            if self.refresh {
                self.draw()?;
            }

            thread::sleep(Duration::from_millis(8 - render_time.elapsed().subsec_millis().min(8) as u64));
        }
    }
}



