use nix::libc;
use nix::pty;

use std::process::{Command, Stdio, Child};
use std::os::unix::process::CommandExt;
use std::os::fd::{FromRawFd, AsRawFd};
use std::io::{Error, ErrorKind};
use std::fs::File;

nix::ioctl_write_ptr_bad!(set_window_size, libc::TIOCSWINSZ, pty::Winsize);


pub struct Pty {
    pub child: Child,
    pub file: File,
}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGHUP);
        }

        let _ = self.child.wait();
    }
}

impl Pty {
    pub fn new() -> Result<Pty, Box<dyn std::error::Error>> {
        let fd = pty::openpty(None, None)?;
        let master = fd.master.as_raw_fd();
        let slave = fd.master.as_raw_fd();

        let mut builder = Command::new("/bin/bash");

        builder.stdin(unsafe { Stdio::from_raw_fd(fd.slave.as_raw_fd()) });
        builder.stdout(unsafe { Stdio::from_raw_fd(fd.slave.as_raw_fd()) });
        builder.stderr(unsafe { Stdio::from_raw_fd(fd.slave.as_raw_fd()) });

        builder.env_remove("LINES");
        builder.env_remove("COLUMNS");

        unsafe {
            builder.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(Error::new(ErrorKind::Other, "failed to set session id"));
                }

                if libc::ioctl(fd.slave.as_raw_fd(), libc::TIOCSCTTY, 0) == -1 {
                    return Err(Error::new(ErrorKind::Other, "ioctl failed"));
                }

                libc::close(slave);
                libc::close(master);

                Ok(())
            });

        }

        let child = builder.spawn()?;

        Ok(Pty {
            child,
            file: File::from(fd.master),
        })
    }

    pub fn resize(&mut self, width: u16, height: u16) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            let winsize = libc::winsize {
                ws_row: height,
                ws_col: width,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };

            set_window_size(self.file.as_raw_fd(), &winsize)?;
        }

        Ok(())
    }
}


