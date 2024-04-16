mod utf8;

use utf8::*;


#[derive(Debug)]
pub enum Action {
    Print(char),
    Execute(u8),
    CsiDispatch(Vec<u16>, Vec<u8>, char),
    EscDispatch(Vec<u8>, u8),
    OscDispatch(Vec<u8>),
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
    csi: Vec<u16>,
    osc: Vec<u8>,
    index: usize,
}

pub struct Parser {
    state: State,
    params: Params,
    intermediates: Vec<u8>,
    utf8: Utf8,
}

impl Parser {
    pub fn new() -> Parser {
        Parser {
            state: State::Anywhere,
            params: Params {
                csi: Vec::new(),
                osc: Vec::new(),
                index: 0,
            },
            intermediates: Vec::new(),
            utf8: Utf8::new(),
        }
    }

    fn reset(&mut self) {
        self.state = State::Anywhere;
        self.params.csi.drain(..);
        self.intermediates.drain(..);
        self.params.index = 0;
    }

    fn esc_param(&mut self, byte: u8) -> Result<Option<Action>, Box<dyn std::error::Error>> {
        // https://www.gnu.org/software/teseq/manual/html_node/Escape-Sequence-Recognition.html

        if (0x30..0x7e).contains(&byte) {
            let action = Action::EscDispatch(self.intermediates.clone(), byte);
            self.reset();

            return Ok(Some(action));
        } else if (0x20..0x2f).contains(&byte) {
            self.intermediates.push(byte);
        }

        Ok(None)
    }

    pub fn advance(&mut self, byte: u8) -> Result<Option<Action>, Box<dyn std::error::Error>> {
        match byte {
            0x1b => {
                self.state = State::Entry;
            },
            _ => {
                match self.state {
                    State::Anywhere => {
                        if (0x00..0x1f).contains(&byte) {
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
                            if let Ok(Some(action)) = self.esc_param(byte) {
                                return Ok(Some(action));
                            } else {
                                self.state = State::EscParams;
                            }
                        }
                    },
                    State::CsiParams => {
                        // https://www.gnu.org/software/teseq/manual/html_node/Escape-Sequence-Recognition.html

                        if (0x40..0x7e).contains(&byte) {
                            let action = Action::CsiDispatch(self.params.csi.clone(), self.intermediates.clone(), byte as char);
                            self.reset();

                            return Ok(Some(action));
                        } else if (0x30..0x3f).contains(&byte) {
                            if byte as char == ';' || byte as char == ':' {
                                if self.params.csi.get(self.params.index).is_some() {
                                    self.params.index += 1;
                                }
                            } else {
                                if let Some(value) = self.params.csi.get(self.params.index) {
                                    self.params.csi[self.params.index] = format!("{}{}", *value, byte - 0x30).parse::<u16>()?;
                                } else {
                                    self.params.csi.insert(self.params.index, byte as u16 - 0x30);
                                }
                            }
                        } else if (0x20..0x2f).contains(&byte) {
                            self.intermediates.push(byte);
                        } else if (0x0..0x0f).contains(&byte) {
                            return Ok(Some(Action::Execute(byte)));
                        }
                    },
                    State::EscParams => {
                        return self.esc_param(byte);
                    },
                    State::OscParams => {
                        if byte == 0x07 || byte == 0x9c {
                            let action = Action::OscDispatch(self.params.osc.clone());
                            self.reset();

                            return Ok(Some(action));
                        } else {
                            self.params.osc.push(byte);
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

    #[test]
    fn escape() {
        let mut parser = Parser::new();

        let bytes = "\u{1b}[1D\u{1b}D+\u{1b}[1D\u{1b}D+\u{1b}[1D\u{1b}D+\u{1b}[1D\u{1b}D+\u{1b}[1D\u{1b}D\u{1b}[23;79H+\u{1b}[1D\u{1b}M+";

        for byte in bytes.as_bytes() {
            // println!("byte: {:?}, state: {:?}", *byte as char, parser.state);

            if let Ok(Some(action)) = parser.advance(*byte) {
                println!("{:?}", action);
            }
        }
    }
}


