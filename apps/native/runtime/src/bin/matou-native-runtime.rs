//! Private native child. Only protocol messages go to stdout.
use std::{io, sync::mpsc, thread};

use anyhow::{Context, Result, bail};
use matou_native_runtime::{
    host::{Command, Event, LaunchOptions, RuntimeHost},
    protocol::{
        ClientMessage, ServerMessage, VERSION, check_version, read_messages, write_message,
    },
};

fn main() -> Result<()> {
    let options = LaunchOptions::from_args(std::env::args_os().skip(1))?;
    // Move this same persistent parser to the reader thread after Hello. Creating
    // another parser would lose any commands already buffered alongside Hello.
    let mut messages = read_messages::<ClientMessage>(io::stdin());
    match messages.next().context("expected native runtime Hello")?? {
        ClientMessage::Hello { version } => check_version(version)?,
        ClientMessage::Command(_) => bail!("first native runtime message must be Hello"),
    }
    let mut stdout = io::stdout().lock();
    write_message(&mut stdout, &ServerMessage::Hello { version: VERSION })?;

    // No runtime, database or PTY exists before valid Hello above. Ready from the
    // real owner, not the protocol Hello, signals successful domain initialization.
    let mut host = RuntimeHost::start(options)?;
    let commands = host.commands.clone();
    let (input_result, receive_input_result) = mpsc::channel();
    let pump_result = (|| -> Result<()> {
        thread::Builder::new()
            .name("matou-runtime-input".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    for message in messages {
                        match message.context("decode native runtime command")? {
                            ClientMessage::Command(Command::Shutdown) => return Ok(()),
                            ClientMessage::Command(command) => commands
                                .send(command)
                                .context("forward native runtime command")?,
                            ClientMessage::Hello { .. } => {
                                bail!("native runtime Hello must not be repeated");
                            }
                        }
                    }
                    Ok(()) // Clean stdin EOF is an orderly shutdown request.
                })();
                // Publish the decode/EOF outcome before the owner can close, so
                // Event::Closed cannot race ahead and hide an input error.
                let _ = input_result.send(result);
                let _ = commands.send(Command::Shutdown);
            })
            .context("start native runtime input reader")?;
        loop {
            let event = host
                .events
                .recv_blocking()
                .context("native runtime event stream ended without Closed")?;
            let closed = matches!(&event, Event::Closed);
            write_message(&mut stdout, &ServerMessage::Event(event))?;
            if closed {
                return Ok(());
            }
        }
    })();
    // Always join the domain owner, including pipe-write or reader-start failure.
    // Its actual error takes precedence over input-forwarding/pipe symptoms.
    host.shutdown()?;
    pump_result?;
    match receive_input_result.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => Ok(()),
        Err(mpsc::TryRecvError::Disconnected) => {
            bail!("native runtime input reader ended without an outcome")
        }
    }
    // Do not join the stdin reader: the host may finish while it is blocked in
    // stdin. Returning exits this dedicated child and ends that I/O thread.
}
