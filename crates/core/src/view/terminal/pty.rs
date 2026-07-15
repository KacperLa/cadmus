use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::Write;
use std::os::fd::{FromRawFd as _, OwnedFd};

pub(super) struct Pty {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
}

impl Pty {
    pub(super) fn spawn(
        shell: Option<&str>,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<Self> {
        let pty_system = native_pty_system();

        let size = PtySize {
            rows,
            cols,
            pixel_width,
            pixel_height,
        };

        let pair = pty_system.openpty(size).context("Failed to open PTY")?;

        let shell_path = shell.unwrap_or("/bin/sh");
        let mut cmd = CommandBuilder::new(shell_path);
        cmd.env("TERM", "xterm-256color");

        pair.slave
            .spawn_command(cmd)
            .context("Failed to spawn shell")?;

        let writer = pair.master.take_writer().context("Failed to get writer")?;

        Ok(Pty {
            writer,
            master: pair.master,
        })
    }

    /// Duplicates the PTY master into an owned fd, safe to register with `AsyncFd`.
    ///
    /// The duplicate is independent of `master`'s lifetime, so the reader job can
    /// own it while the session keeps `Pty` for writes.
    pub(super) fn take_reader_fd(&self) -> Result<OwnedFd> {
        // SAFETY: `as_raw_fd` returns the live, owned PTY master fd; the
        // duplicate created here is an independent file description.
        let raw = self
            .master
            .as_raw_fd()
            .context("PTY master has no file descriptor")?;
        // nix 0.31 has no safe `dup` that takes a raw fd; the libc call is
        // sound because the raw fd is owned by `self.master` and open.
        match unsafe { libc::dup(raw) } {
            fd if fd >= 0 => Ok(unsafe { OwnedFd::from_raw_fd(fd) }),
            _ => Err(std::io::Error::last_os_error()).context("Failed to duplicate PTY master fd"),
        }
    }

    pub(super) fn write(&mut self, data: &[u8]) -> Result<usize> {
        let n = self.writer.write(data).context("PTY write failed")?;
        self.writer.flush().context("PTY flush failed")?;
        Ok(n)
    }

    pub(super) fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<()> {
        let size = PtySize {
            rows,
            cols,
            pixel_width,
            pixel_height,
        };
        self.master.resize(size).context("PTY resize failed")?;
        Ok(())
    }
}
