#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod gui;

use std::{error::Error, process::ExitCode};

use tagcast_player::{APP_NAME, account::logout};

fn main() -> ExitCode {
    #[cfg(windows)]
    if std::env::args_os().len() > 1 {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

        // CLI output can use the caller's console. Explorer launches never need a new one.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{APP_NAME}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut client_id = std::env::var("IBROADCAST_CLIENT_ID")
        .ok()
        .filter(|id| !id.is_empty());
    let mut forget = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "{APP_NAME}: a lightweight, read-only iBroadcast desktop player\n\nUsage: tagcast-player [--client-id ID] [--logout]\n\nOn first launch, sign in with the device code shown in the window.\n--logout removes this player's stored sign-in and library snapshot."
                );
                return Ok(());
            }
            "-V" | "--version" => {
                println!("tagcast-player {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--client-id" => client_id = Some(args.next().ok_or("--client-id requires an ID")?),
            "--logout" => forget = true,
            // macOS passes a process serial number when an app bundle is opened from Finder.
            other if other.starts_with("-psn_") => {}
            _ => return Err("Unknown argument. See --help".into()),
        }
    }
    if let Some(id) = &client_id
        && (id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'))
    {
        return Err("Invalid client ID".into());
    }
    if forget {
        logout(client_id)?;
        println!("Cleared this player's local sign-in and library snapshot.");
        return Ok(());
    }
    gui::run(gui::Options { client_id })
}
