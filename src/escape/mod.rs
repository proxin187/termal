mod utf8;

use utf8::*;


const MAX_INTERMEDIATES: usize = 2;
const MAX_CSI: usize = 128;


#[derive(Debug)]
pub enum Action<'a> {
    Print(char),
    Execute(u8),
    CsiDispatch(&'a [u16], &'a [u8], char),
    EscDispatch(&'a [u8], u8),
    OscDispatch(&'a [u8]),
}

#[derive(Debug)]
pub enum State {
    Anywhere,
    Entry,
    CsiParams,
    EscParams,
    OscParams,
}

pub struct Params {
    csi: [u16; MAX_CSI],
    osc: [u8; 1024],
    index: usize,
}

pub struct Intermediates {
    buf: [u8; MAX_INTERMEDIATES],
    index: usize,
}

impl Intermediates {
    fn esc_param(&mut self, byte: u8, state: &mut State) -> Result<Option<Action>, Box<dyn std::error::Error>> {
        // https://www.gnu.org/software/teseq/manual/html_node/Escape-Sequence-Recognition.html

        if byte >= 0x30 && byte <= 0x7e {
            let action = Action::EscDispatch(&self.buf[..self.index], byte);

            *state = State::Anywhere;

            self.index = 0;

            return Ok(Some(action));
        } else if byte >= 0x20 && byte <= 0x2f {
            self.buf[self.index] = byte;

            self.index += 1;
        }

        Ok(None)
    }
}

pub struct Parser {
    state: State,
    params: Params,
    intermediates: Intermediates,
    utf8: Utf8,
}

impl<'a> Parser {
    pub fn new() -> Parser {
        Parser {
            state: State::Anywhere,
            params: Params {
                csi: [0; MAX_CSI],
                osc: [0; 1024],
                index: 0,
            },
            intermediates: Intermediates {
                buf: [0; MAX_INTERMEDIATES],
                index: 0,
            },
            utf8: Utf8::new(),
        }
    }

    pub fn advance(&'a mut self, byte: u8) -> Result<Option<Action>, Box<dyn std::error::Error>> {
        match byte {
            0x1b => {
                self.intermediates.index = 0;
                self.params.index = 0;

                self.intermediates.buf = [0; MAX_INTERMEDIATES];
                self.params.csi = [0; MAX_CSI];

                self.state = State::Entry;
            },
            _ => {
                match self.state {
                    State::Anywhere => {
                        if byte < 0x1f {
                            return Ok(Some(Action::Execute(byte)));
                        } else {
                            if let Some(c) = self.utf8.advance(byte) {
                                match c {
                                    Codepoint::Valid(c) => {
                                        return Ok(Some(Action::Print(c)));
                                    },
                                    Codepoint::Invalid => {
                                        println!("[+] invalid codepoint");
                                    },
                                }
                            }
                        }
                    },
                    State::Entry => {
                        if byte as char == '[' {
                            self.state = State::CsiParams;
                        } else if byte as char == ']' {
                            self.state = State::OscParams;
                        } else {
                            if let Ok(Some(action)) = self.intermediates.esc_param(byte, &mut self.state) {
                                return Ok(Some(action));
                            } else {
                                self.state = State::EscParams;
                            }
                        }
                    },
                    State::CsiParams => {
                        // https://www.gnu.org/software/teseq/manual/html_node/Escape-Sequence-Recognition.html

                        if byte >= 0x40 && byte < 0x7e {
                            let action = Action::CsiDispatch(
                                &self.params.csi[..=self.params.index],
                                &self.intermediates.buf[..self.intermediates.index],
                                byte as char
                            );

                            self.state = State::Anywhere;

                            return Ok(Some(action));
                        } else if byte >= 0x30 && byte < 0x3f {
                            if byte as char == ';' || byte as char == ':' {
                                self.params.index += 1;
                            } else {
                                if self.params.csi[self.params.index] != 0 {
                                    self.params.csi[self.params.index] = ((self.params.csi[self.params.index] as usize * 10) + byte as usize - 0x30).min(u16::MAX as usize) as u16;
                                } else {
                                    self.params.csi[self.params.index] = byte as u16 - 0x30;
                                }
                            }
                        } else if byte >= 0x20 && byte < 0x2f && self.intermediates.index <= MAX_INTERMEDIATES {
                            self.intermediates.buf[self.intermediates.index] = byte;

                            self.intermediates.index += 1;
                        } else if byte < 0x0f {
                            return Ok(Some(Action::Execute(byte)));
                        }
                    },
                    State::EscParams => {
                        return self.intermediates.esc_param(byte, &mut self.state);
                    },
                    State::OscParams => {
                        if byte == 0x07 || byte == 0x9c {
                            let action = Action::OscDispatch(&self.params.osc[..self.params.index]);

                            self.state = State::Anywhere;

                            return Ok(Some(action));
                        } else {
                            self.params.osc[self.params.index] = byte;

                            self.params.index += 1;
                        }
                    },
                }
            },
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Performer {}

    impl vte::Perform for Performer {}

    #[test]
    fn benchmark() -> Result<(), Box<dyn std::error::Error>> {
        let demo = b"\x1b[3;1\x1b[?1049h";

        let mut termal_log = 0.0;
        let mut vte_log = 0.0;

        for _ in 0..=100 {
            let mut performer = Performer {};
            let mut vte = vte::Parser::new();
            let mut parser = Parser::new();

            let termal_start = std::time::Instant::now();

            for byte in demo {
                parser.advance(*byte)?;
            }

            let termal_bench = termal_start.elapsed().as_secs_f64();

            let vte_start = std::time::Instant::now();

            for byte in demo {
                vte.advance(&mut performer, *byte);
            }

            let vte_bench = vte_start.elapsed().as_secs_f64();

            termal_log += termal_bench;
            vte_log += vte_bench;
        }

        println!("[+] vte: {}", vte_log / 100.0);
        println!("[+] termal: {}", termal_log / 100.0);

        if vte_log < termal_log {
            println!("[+] vte was faster by {}", termal_log - vte_log);
        } else {
            println!("[+] termal was faster by {}", vte_log - termal_log);
        }

        Ok(())
    }

    #[test]
    fn vt100() {
        let mut performer = Performer {};
        let mut vte = vte::Parser::new();

        let bytes = b"\x1b[2;;2;6;;2A";

        for byte in bytes {
            vte.advance(&mut performer, *byte);
        }
    }

    #[test]
    fn escape() {
        let mut parser = Parser::new();

        let bytes = "\u{1b}[1D\u{1b}D+\u{1b}[1D\u{1b}D+\u{1b}[1D\u{1b}D+\u{1b}[1D\u{1b}D+\u{1b}[1D\u{1b}D\u{1b}[23;79H+\u{1b}[1D\u{1b}M+";

        for byte in bytes.as_bytes() {
            // println!("byte: {:?}, state: {:?}", *byte as char, parser.state);

            if let Ok(Some(action)) = parser.advance(*byte) {
                // println!("{:?}", action);
            }
        }
    }
}


