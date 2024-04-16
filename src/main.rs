mod terminal;
mod escape;
mod config;
mod xlib;
mod pty;

use terminal::Terminal;

use std::process;


fn main() {
    let mut terminal = match Terminal::new() {
        Ok(terminal) => terminal,
        Err(err) => {
            println!("[+] failed to create terminal: {}", err);
            process::exit(1);
        },
    };

    if let Err(err) = terminal.run() {
        println!("[+] terminal failed: {}", err);
        process::exit(1);
    }

    /*
    let bytes = b"before\x1b[22A\nbetween\x1b[54;9H\x1b$(Cafter";
    let mut parser = escape::Parser::new();

    for byte in bytes {
        if let Ok(Some(action)) = parser.advance(*byte) {
            println!("action: {:?}", action);
        }
    }
    */
}

