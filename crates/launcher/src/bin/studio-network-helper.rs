#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
fn main() {
    let result: Result<(studio_network::windows::CancelWatch, Result<String, String>), String> =
        (|| {
            let args: Vec<_> = std::env::args().skip(1).collect();
            if args.len() != 6
                || args[0] != "--parent"
                || args[2] != "--request"
                || args[3].len() > 32768
                || args[4] != "--token"
            {
                return Err("Use ROS Studio's Network settings to manage access".into());
            }
            let parent = args[1].parse::<u32>().map_err(|e| e.to_string())?;
            let request = studio_network::firewall::Request::decode(
                &hex::decode(&args[3]).map_err(|e| e.to_string())?,
            )?;
            let watch = studio_network::windows::watch_cancel(parent, &args[5])?;
            Ok((
                watch,
                studio_launcher::firewall_helper::run(parent, &request),
            ))
        })();
    match result {
        Ok((_watch, Ok(text))) => studio_launcher::firewall_helper::message(&text, false),
        Ok((_watch, Err(error))) => {
            studio_launcher::firewall_helper::message(&error, true);
            std::process::exit(1);
        }
        Err(error) => {
            studio_launcher::firewall_helper::message(&error, true);
            std::process::exit(1);
        }
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Windows-only network helper");
    std::process::exit(1);
}
