mod terminal;
mod escape;
mod config;
mod xlib;
mod pty;

use terminal::Terminal;

use std::process;


fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    Ok(())
}

