

#[derive(Debug)]
pub enum Codepoint {
    Valid(char),
    Invalid,
}

#[derive(Debug)]
pub enum Action {
    Emit(u8),
    SetByte1(u8),
    SetByte2(u8),
    SetByte2Top(u8),
    SetByte3(u8),
    SetByte3Top(u8),
    SetByte4Top(u8),
    InvalidSequence,
}

#[derive(Debug)]
pub enum State {
    Ground,
    Tail1,
    Tail2,
    Tail3,
}

impl State {
    pub fn advance(&mut self, byte: u8) -> Action {
        match self {
            State::Ground => {
                match byte {
                    // 1 byte
                    0x00..=0x7f => {
                        *self = State::Ground;

                        Action::Emit(byte)
                    },
                    // 2 byte
                    0xc2..=0xdf => {
                        *self = State::Tail1;

                        Action::SetByte2Top(byte)
                    },
                    // 3 byte
                    0xe0..=0xef => {
                        *self = State::Tail2;

                        Action::SetByte3Top(byte)
                    },
                    // 4 byte
                    0xf0..=0xf4 => {
                        *self = State::Tail3;

                        Action::SetByte4Top(byte)
                    },
                    _ => Action::InvalidSequence,
                }
            },
            State::Tail3 => {
                *self = State::Tail2;

                Action::SetByte3(byte)
            },
            State::Tail2 => {
                *self = State::Tail1;

                Action::SetByte2(byte)
            },
            State::Tail1 => {
                *self = State::Ground;

                Action::SetByte1(byte)
            },
        }
    }
}

pub struct Utf8 {
    state: State,
    point: u32,
}

impl Utf8 {
    pub fn new() -> Utf8 {
        Utf8 {
            state: State::Ground,
            point: 0,
        }
    }

    pub fn advance(&mut self, byte: u8) -> Option<Codepoint> {
        match self.state.advance(byte) {
            Action::Emit(byte) => {
                self.point = 0;

                return Some(Codepoint::Valid(byte as char));
            },
            Action::SetByte1(byte) => {
                let point = self.point | (byte as u32) & 0b0011_1111;

                self.point = 0;

                return Some(Codepoint::Valid(char::from_u32(point).unwrap()));
            },
            Action::SetByte2(byte) => self.point |= (byte as u32 & 0b0011_1111) << 6,
            Action::SetByte3(byte) => self.point |= (byte as u32 & 0b0011_1111) << 12,
            Action::SetByte2Top(byte) => self.point |= (byte as u32 & 0b0001_1111) << 6,
            Action::SetByte3Top(byte) => self.point |= (byte as u32 & 0b0000_1111) << 12,
            Action::SetByte4Top(byte) => self.point |= (byte as u32 & 0b0000_0111) << 18,
            Action::InvalidSequence => return Some(Codepoint::Invalid),
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_test() {
        let text = "hello Ж ✳ 𝄞 ─";

        let mut utf8 = Utf8::new();

        for byte in text.bytes() {
            // println!("{:?}", utf8.advance(byte));
        }
    }
}


