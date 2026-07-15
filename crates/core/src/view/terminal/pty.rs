use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::os::unix::io::RawFd;

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

    pub(super) fn take_reader(&self) -> Result<Box<dyn Read + Send>> {
        self.master
            .try_clone_reader()
            .context("Failed to create reader")
    }

    pub(super) fn as_raw_fd(&self) -> Option<RawFd> {
        self.master.as_raw_fd()
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
